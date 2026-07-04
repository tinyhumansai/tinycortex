//! Git-backed persistence for memory diff snapshots, checkpoints, and read
//! markers — the diff *ledger*.
//!
//! The ledger is a libgit2 repository at `<workspace>/memory_diff/repo`. It is
//! a *derived* view of the chunk store — the chunk store remains the
//! authoritative source of memory. Each snapshot materialises a source's
//! current items as blobs under an encoded source-id directory and records them
//! as a commit; the rest of the tree (other sources) is carried forward from
//! the parent so HEAD always reflects the whole world. This maps the diff
//! domain onto git's native primitives:
//!
//! - **Snapshot**   → commit (`Snapshot.id` is the commit SHA)
//! - **Checkpoint** → annotated tag `ckpt_<uuid>` at HEAD
//! - **Read marker**→ ref `refs/openhuman/read/<encoded_source_id>` → commit SHA
//! - **Diff**       → `git diff <from-tree>..<to-tree>` scoped to the source path
//!
//! Item identity is the file name: each item is one flat blob whose name is the
//! item id encoded into a git-safe path component ([`encode_item_id`]). A
//! content change keeps the same name → `Modified`; renaming the item id is
//! `Removed` + `Added`, matching the id-keyed semantics.
//!
//! Snapshot metadata that has no natural git home (source kind/label, trigger,
//! item count, millisecond timestamp) rides in the commit message as trailers.
//! All mutations serialise through a process-global [`WRITE_LOCK`] because the
//! repository's parent/HEAD bookkeeping is not safe to interleave.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use anyhow::{Context, Result};
use git2::{Delta, DiffOptions, Object, ObjectType, Oid, Repository, Signature, Time};

use super::types::{ChangeKind, Checkpoint, DiffSummary, ItemChange, Snapshot, SnapshotTrigger};

/// Serialises all writes (commits, tags, ref updates) to the ledger. libgit2's
/// HEAD/parent resolution is read-modify-write, so concurrent commits could
/// otherwise fork history or lose a snapshot.
static WRITE_LOCK: Mutex<()> = Mutex::new(());

const BLOB_MODE: i32 = 0o100644;
const TREE_MODE: i32 = 0o040000;
const SIG_NAME: &str = "TinyCortex Memory";
const SIG_EMAIL: &str = "memory-diff@tinycortex.local";
const READ_MARKER_PREFIX: &str = "refs/openhuman/read/";
const CHECKPOINT_PREFIX: &str = "ckpt_";

/// Upper bound on a single modified-item unified diff embedded in `text_diff`.
const MAX_TEXT_DIFF_CHARS: usize = 2000;

// ── Repository handle ──────────────────────────────────────────────────

/// A handle to the diff ledger. Cheap to open; callers construct one per
/// operation.
pub struct Ledger {
    repo: Repository,
}

/// Metadata describing the snapshot a commit represents. Persisted as commit
/// trailers and reconstructed by [`Ledger::snapshot_from_commit`].
pub struct SnapshotMeta {
    /// Logical source id.
    pub source_id: String,
    /// Source kind wire string.
    pub source_kind: String,
    /// Human-readable label.
    pub label: String,
    /// Why the snapshot was taken.
    pub trigger: SnapshotTrigger,
}

impl Ledger {
    /// Open the ledger, initialising the repository on first use.
    pub fn open(workspace_dir: &Path) -> Result<Self> {
        let repo_path = workspace_dir.join("memory_diff").join("repo");
        std::fs::create_dir_all(&repo_path)
            .with_context(|| format!("create memory_diff repo dir: {}", repo_path.display()))?;

        let repo = match Repository::open(&repo_path) {
            Ok(repo) => repo,
            Err(_) => Repository::init(&repo_path)
                .with_context(|| format!("init memory_diff repo: {}", repo_path.display()))?,
        };
        Ok(Self { repo })
    }

    // ── Snapshots (commits) ────────────────────────────────────────────

    /// Commit a snapshot for one source: replace the source's subtree with the
    /// given items (each `(item_id, content)`), carrying every other source
    /// forward from the parent. Returns the resulting [`Snapshot`].
    pub fn commit_snapshot(
        &self,
        meta: &SnapshotMeta,
        items: &[(String, String)],
        taken_at_ms: i64,
    ) -> Result<Snapshot> {
        let _guard = WRITE_LOCK.lock().expect("memory_diff write lock poisoned");

        // Build the source subtree from scratch: one blob per item.
        let source_tree_oid = {
            let mut tb = self.repo.treebuilder(None)?;
            for (item_id, content) in items {
                let blob = self.repo.blob(content.as_bytes())?;
                tb.insert(encode_item_id(item_id), blob, BLOB_MODE)?;
            }
            tb.write()?
        };

        // Start the root tree from the parent commit (carry other sources),
        // then graft in the new source subtree (or drop it if empty).
        let parent_commit = match self.repo.head() {
            Ok(head) => Some(head.peel_to_commit()?),
            Err(_) => None, // unborn HEAD on a fresh repo
        };
        let parent_root = match &parent_commit {
            Some(c) => Some(c.tree()?),
            None => None,
        };
        let source_path = encode_source_id(&meta.source_id);
        let root_oid = {
            let mut tb = self.repo.treebuilder(parent_root.as_ref())?;
            if items.is_empty() {
                if tb.get(source_path.as_str())?.is_some() {
                    tb.remove(source_path.as_str())?;
                }
            } else {
                tb.insert(source_path.as_str(), source_tree_oid, TREE_MODE)?;
            }
            tb.write()?
        };
        let tree = self.repo.find_tree(root_oid)?;

        let message = build_commit_message(meta, items.len() as u32, taken_at_ms);
        let sig = signature(taken_at_ms)?;
        let parents: Vec<&git2::Commit> = parent_commit.iter().collect();
        let commit_oid = self
            .repo
            .commit(Some("HEAD"), &sig, &sig, &message, &tree, &parents)
            .context("write snapshot commit")?;

        Ok(Snapshot {
            id: commit_oid.to_string(),
            source_id: meta.source_id.clone(),
            source_kind: meta.source_kind.clone(),
            label: meta.label.clone(),
            trigger: meta.trigger.clone(),
            item_count: items.len() as u32,
            taken_at_ms,
        })
    }

    /// List snapshots newest-first, optionally filtered to one source.
    ///
    /// Walks the commit history from HEAD; each commit is one source's
    /// snapshot, identified by its `Source-Id` trailer.
    pub fn list_snapshots(&self, source_id: Option<&str>, limit: u32) -> Result<Vec<Snapshot>> {
        let mut walk = match self.repo.revwalk() {
            Ok(w) => w,
            Err(_) => return Ok(Vec::new()),
        };
        if walk.push_head().is_err() {
            // Unborn HEAD → no snapshots yet.
            return Ok(Vec::new());
        }
        walk.set_sorting(git2::Sort::TIME)?;

        let mut out = Vec::new();
        for oid in walk {
            let oid = oid?;
            let commit = self.repo.find_commit(oid)?;
            let snap = self.snapshot_from_commit(&commit);
            if let Some(filter) = source_id {
                if snap.source_id != filter {
                    continue;
                }
            }
            out.push(snap);
            if out.len() as u32 >= limit {
                break;
            }
        }
        Ok(out)
    }

    /// Fetch a single snapshot by commit SHA, if it exists.
    pub fn get_snapshot(&self, snapshot_id: &str) -> Result<Option<Snapshot>> {
        let Ok(oid) = Oid::from_str(snapshot_id) else {
            return Ok(None);
        };
        match self.repo.find_commit(oid) {
            Ok(commit) => Ok(Some(self.snapshot_from_commit(&commit))),
            Err(_) => Ok(None),
        }
    }

    /// The `count` most recent snapshots for a source, newest-first.
    pub fn latest_snapshots_for_source(
        &self,
        source_id: &str,
        count: u32,
    ) -> Result<Vec<Snapshot>> {
        self.list_snapshots(Some(source_id), count)
    }

    /// Number of snapshots a source has.
    pub fn snapshot_count_for_source(&self, source_id: &str) -> Result<usize> {
        Ok(self.list_snapshots(Some(source_id), u32::MAX)?.len())
    }

    // ── Diff (tree-to-tree) ─────────────────────────────────────────────

    /// Compute item-level changes for `source_id` between two snapshots.
    ///
    /// `from` is `None` for a first-ever diff (everything added). Both commits
    /// must belong to `source_id`; cross-source mixing is rejected by the
    /// caller before reaching here.
    pub fn compute_changes(
        &self,
        from: Option<&str>,
        to: &str,
        source_id: &str,
        to_item_count: u32,
        include_text_diff: bool,
    ) -> Result<(Vec<ItemChange>, DiffSummary)> {
        let to_oid = Oid::from_str(to).with_context(|| format!("bad to snapshot id: {to}"))?;
        let to_tree = self.repo.find_commit(to_oid)?.tree()?;

        let from_tree = match from {
            Some(f) => {
                let oid = Oid::from_str(f).with_context(|| format!("bad from snapshot id: {f}"))?;
                Some(self.repo.find_commit(oid)?.tree()?)
            }
            None => None,
        };

        let encoded_source_id = encode_source_id(source_id);
        let path_prefix = format!("{encoded_source_id}/");
        let mut opts = DiffOptions::new();
        opts.pathspec(&encoded_source_id);
        opts.context_lines(3);
        let diff =
            self.repo
                .diff_tree_to_tree(from_tree.as_ref(), Some(&to_tree), Some(&mut opts))?;

        let mut changes = Vec::new();
        let mut summary = DiffSummary::default();

        for (idx, delta) in diff.deltas().enumerate() {
            // Resolve the item path; guard against pathspec prefix overreach
            // (e.g. "src_a" must not match "src_abc/...").
            let path = delta
                .new_file()
                .path()
                .or_else(|| delta.old_file().path())
                .and_then(|p| p.to_str())
                .unwrap_or("");
            let Some(encoded) = path.strip_prefix(&path_prefix) else {
                continue;
            };
            let item_id = decode_item_id(encoded);

            let new_oid = delta.new_file().id();
            let old_oid = delta.old_file().id();

            let (kind, title) = match delta.status() {
                Delta::Added | Delta::Copied | Delta::Untracked => {
                    summary.added += 1;
                    (ChangeKind::Added, self.title_for(&item_id, new_oid))
                }
                Delta::Deleted => {
                    summary.removed += 1;
                    (ChangeKind::Removed, self.title_for(&item_id, old_oid))
                }
                Delta::Modified | Delta::Renamed | Delta::Typechange => {
                    summary.modified += 1;
                    (ChangeKind::Modified, self.title_for(&item_id, new_oid))
                }
                // Unmodified / ignored / conflicted: nothing to report.
                _ => continue,
            };

            let text_diff = if include_text_diff && kind == ChangeKind::Modified {
                patch_text(&diff, idx)
            } else {
                None
            };

            changes.push(ItemChange {
                item_id,
                title,
                kind,
                old_content_hash: oid_hash(old_oid),
                new_content_hash: oid_hash(new_oid),
                text_diff,
            });
        }

        // git only reports changed entries; unchanged = everything in `to`
        // that wasn't added or modified.
        summary.unchanged = to_item_count
            .saturating_sub(summary.added)
            .saturating_sub(summary.modified);

        Ok((changes, summary))
    }

    // ── Read markers (refs) ─────────────────────────────────────────────

    /// The commit SHA a source's read marker points at, if set.
    pub fn get_read_marker(&self, source_id: &str) -> Result<Option<String>> {
        let name = read_marker_ref(source_id);
        match self.repo.find_reference(&name) {
            Ok(r) => Ok(r.target().map(|o| o.to_string())),
            Err(_) => Ok(None),
        }
    }

    /// Set (or advance) a source's read marker to a commit SHA.
    pub fn set_read_marker(&self, source_id: &str, snapshot_id: &str) -> Result<()> {
        let _guard = WRITE_LOCK.lock().expect("memory_diff write lock poisoned");
        let oid = Oid::from_str(snapshot_id)
            .with_context(|| format!("bad read-marker snapshot id: {snapshot_id}"))?;
        let name = read_marker_ref(source_id);
        self.repo
            .reference(&name, oid, true, "advance memory_diff read marker")
            .with_context(|| format!("set read marker ref: {name}"))?;
        Ok(())
    }

    // ── Checkpoints (tags) ──────────────────────────────────────────────

    /// Create an annotated tag at HEAD recording a checkpoint. The label and
    /// per-source head snapshot ids ride in the tag message as JSON.
    pub fn create_checkpoint(
        &self,
        id: &str,
        label: &str,
        snapshot_ids: &[String],
        created_at_ms: i64,
    ) -> Result<()> {
        let _guard = WRITE_LOCK.lock().expect("memory_diff write lock poisoned");
        let head = self
            .repo
            .head()
            .context("checkpoint requires at least one snapshot")?
            .peel_to_commit()?;
        let target: Object = head.into_object();
        let sig = signature(created_at_ms)?;
        let message = checkpoint_message(label, snapshot_ids, created_at_ms);
        self.repo
            .tag(id, &target, &sig, &message, true)
            .with_context(|| format!("create checkpoint tag: {id}"))?;
        Ok(())
    }

    /// Load a checkpoint by tag name.
    pub fn get_checkpoint(&self, checkpoint_id: &str) -> Result<Option<Checkpoint>> {
        let refname = format!("refs/tags/{checkpoint_id}");
        let Ok(reference) = self.repo.find_reference(&refname) else {
            return Ok(None);
        };
        let obj = reference.peel(ObjectType::Tag).ok();
        let Some(tag) = obj.and_then(|o| o.into_tag().ok()) else {
            return Ok(None);
        };
        Ok(Some(checkpoint_from_message(
            checkpoint_id,
            // git2 0.21: Tag::message() is Result<Option<&str>, _>; a non-UTF8
            // or missing message degrades to an empty checkpoint body.
            tag.message().ok().flatten().unwrap_or(""),
        )))
    }

    /// List checkpoints newest-first, up to `limit`.
    pub fn list_checkpoints(&self, limit: u32) -> Result<Vec<Checkpoint>> {
        let pattern = format!("{CHECKPOINT_PREFIX}*");
        let names = self.repo.tag_names(Some(&pattern))?;
        let mut out = Vec::new();
        // git2 0.21: StringArray::iter() yields Result<Option<&str>, _>; keep
        // only successfully-decoded utf8 names.
        for name in names.iter().filter_map(|r| r.ok().flatten()) {
            if let Some(ckpt) = self.get_checkpoint(name)? {
                out.push(ckpt);
            }
        }
        out.sort_by_key(|c| std::cmp::Reverse(c.created_at_ms));
        out.truncate(limit as usize);
        Ok(out)
    }

    /// Delete checkpoint tags created before `older_than_ms`. Snapshot commits
    /// are retained — git history is the ledger — so this only prunes named
    /// baselines. Returns the number of tags deleted.
    pub fn cleanup_checkpoints(&self, older_than_ms: i64) -> Result<u64> {
        let _guard = WRITE_LOCK.lock().expect("memory_diff write lock poisoned");
        let pattern = format!("{CHECKPOINT_PREFIX}*");
        let names = self.repo.tag_names(Some(&pattern))?;
        let mut deleted = 0u64;
        // git2 0.21: StringArray::iter() yields Result<Option<&str>, _>.
        for name in names.iter().filter_map(|r| r.ok().flatten()) {
            if let Some(ckpt) = self.get_checkpoint(name)? {
                if ckpt.created_at_ms < older_than_ms {
                    self.repo.tag_delete(name)?;
                    deleted += 1;
                }
            }
        }
        Ok(deleted)
    }

    // ── Helpers ─────────────────────────────────────────────────────────

    /// Reconstruct a [`Snapshot`] from a commit's trailers, falling back to
    /// the commit time when a millisecond trailer is absent.
    fn snapshot_from_commit(&self, commit: &git2::Commit) -> Snapshot {
        let trailers = parse_trailers(commit.message().unwrap_or(""));
        let taken_at_ms = trailers
            .get("taken-at-ms")
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or_else(|| commit.time().seconds() * 1000);
        Snapshot {
            id: commit.id().to_string(),
            source_id: trailers.get("source-id").cloned().unwrap_or_default(),
            source_kind: trailers.get("source-kind").cloned().unwrap_or_default(),
            label: trailers.get("source-label").cloned().unwrap_or_default(),
            trigger: match trailers.get("trigger").map(String::as_str) {
                Some("manual") => SnapshotTrigger::Manual,
                _ => SnapshotTrigger::Auto,
            },
            item_count: trailers
                .get("item-count")
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(0),
            taken_at_ms,
        }
    }

    /// Derive a display title from a blob's content. Returns the item id when
    /// the blob is missing or yields no usable line.
    fn title_for(&self, item_id: &str, oid: Oid) -> String {
        if oid.is_zero() {
            return item_id.to_string();
        }
        match self.repo.find_blob(oid) {
            Ok(blob) => {
                let content = String::from_utf8_lossy(blob.content());
                derive_title(item_id, &content)
            }
            Err(_) => item_id.to_string(),
        }
    }
}

// ── Free helpers ───────────────────────────────────────────────────────

fn signature(at_ms: i64) -> Result<Signature<'static>> {
    let time = Time::new(at_ms / 1000, 0);
    Signature::new(SIG_NAME, SIG_EMAIL, &time).context("build git signature")
}

fn read_marker_ref(source_id: &str) -> String {
    format!("{READ_MARKER_PREFIX}{}", encode_source_id(source_id))
}

fn build_commit_message(meta: &SnapshotMeta, item_count: u32, taken_at_ms: i64) -> String {
    format!(
        "snapshot: {source} ({count} item(s))\n\n\
         Source-Id: {source}\n\
         Source-Kind: {kind}\n\
         Source-Label: {label}\n\
         Trigger: {trigger}\n\
         Item-Count: {count}\n\
         Taken-At-Ms: {taken}\n",
        source = meta.source_id,
        kind = meta.source_kind,
        label = sanitize_trailer(&meta.label),
        trigger = meta.trigger.as_str(),
        count = item_count,
        taken = taken_at_ms,
    )
}

/// Trailer values are single-line; collapse newlines so a multi-line label
/// can't corrupt the trailer block.
fn sanitize_trailer(s: &str) -> String {
    s.replace(['\n', '\r'], " ")
}

/// Parse `Key: value` trailer lines from a commit message into a
/// lowercase-keyed map.
fn parse_trailers(message: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in message.lines() {
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim().to_ascii_lowercase();
            if !key.is_empty() && !key.contains(' ') {
                map.insert(key, v.trim().to_string());
            }
        }
    }
    map
}

fn checkpoint_message(label: &str, snapshot_ids: &[String], created_at_ms: i64) -> String {
    let payload = serde_json::json!({
        "label": label,
        "snapshot_ids": snapshot_ids,
        "created_at_ms": created_at_ms,
    });
    payload.to_string()
}

fn checkpoint_from_message(id: &str, message: &str) -> Checkpoint {
    let value: serde_json::Value = serde_json::from_str(message.trim()).unwrap_or_default();
    Checkpoint {
        id: id.to_string(),
        label: value
            .get("label")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        created_at_ms: value
            .get("created_at_ms")
            .and_then(|v| v.as_i64())
            .unwrap_or(0),
        snapshot_ids: value
            .get("snapshot_ids")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// A git blob oid as a content hash, or `None` for the zero oid (absent side).
fn oid_hash(oid: Oid) -> Option<String> {
    if oid.is_zero() {
        None
    } else {
        Some(oid.to_string())
    }
}

/// Render a single delta's unified patch, truncated to [`MAX_TEXT_DIFF_CHARS`].
fn patch_text(diff: &git2::Diff, delta_idx: usize) -> Option<String> {
    let mut patch = git2::Patch::from_diff(diff, delta_idx).ok().flatten()?;
    let buf = patch.to_buf().ok()?;
    // git2 0.21: Buf::as_str() returns Result<&str, Utf8Error>.
    let text = buf.as_str().ok()?;
    if text.trim().is_empty() {
        None
    } else {
        Some(truncate(text, MAX_TEXT_DIFF_CHARS))
    }
}

/// Encode an item id into a single git-safe path component. Bytes outside
/// `[A-Za-z0-9._-]` become `%XX`; an `i_` prefix keeps the result clear of the
/// reserved names `.`/`..`/empty. Reversible via [`decode_item_id`].
pub(crate) fn encode_item_id(item_id: &str) -> String {
    let mut out = String::with_capacity(item_id.len() + 2);
    out.push_str("i_");
    for &b in item_id.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-') {
            out.push(b as char);
        } else {
            out.push('%');
            out.push_str(&format!("{b:02X}"));
        }
    }
    out
}

/// Encode a source id for use as a top-level git tree entry and read-marker
/// ref component. Same reversible encoding as item ids; kept as a named helper
/// so call sites make the source-vs-item boundary explicit.
pub(crate) fn encode_source_id(source_id: &str) -> String {
    encode_item_id(source_id)
}

/// Inverse of [`encode_item_id`].
pub(crate) fn decode_item_id(encoded: &str) -> String {
    let body = encoded.strip_prefix("i_").unwrap_or(encoded);
    let bytes = body.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn truncate(s: &str, max_chars: usize) -> String {
    if s.len() <= max_chars {
        s.to_string()
    } else {
        let mut end = max_chars;
        while !s.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        format!("{}…(truncated)", &s[..end])
    }
}

/// Derive a human-readable title from item content: the first non-empty line
/// (Markdown heading markers stripped), bounded. Falls back to the item id.
fn derive_title(item_id: &str, content: &str) -> String {
    let first_line = content
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(|l| l.trim_start_matches('#').trim());
    match first_line {
        Some(l) if !l.is_empty() => truncate(l, 120),
        _ => item_id.to_string(),
    }
}

#[cfg(test)]
#[path = "ledger_tests.rs"]
mod tests;
