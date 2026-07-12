//! On-disk archive of raw provider items (one `.md` per source item).
//!
//! Writes a separate tree at
//! `<content_root>/raw/<source_slug>/<kind>/<created_at_ms>_<uid>.md`, where
//! `<kind>` is one of `emails`, `chats`, `documents`, `contacts`, `posts`, … —
//! the verbatim payload captured at sync time (no chunking, no summarisation).
//!
//! Each file is written atomically (tempfile + rename). Re-writing the same
//! `(source, uid, ts)` triple is idempotent.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::paths::slugify_source_id;

/// Category of a raw item, selecting the per-kind subdirectory under
/// `raw/<source_slug>/<kind>/`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum RawKind {
    /// Email messages (Gmail, Outlook, …).
    Email,
    /// Chat / DM messages (Slack, Telegram, WhatsApp, Discord, …).
    Chat,
    /// Standalone documents — Notion pages, Drive files, attachments.
    Document,
    /// One file per person reachable via this source.
    Contact,
    /// Long-form posts — LinkedIn posts, tweets, blog entries.
    Post,
    /// Git commits (one file per commit) — GitHub repo sources.
    Commit,
    /// Issues with their conversation + metadata — GitHub repo sources.
    Issue,
    /// Pull requests with their body + metadata — GitHub repo sources.
    PullRequest,
}

impl RawKind {
    /// Directory name used on disk for this kind (plural).
    pub const fn as_dir(&self) -> &'static str {
        match self {
            Self::Email => "emails",
            Self::Chat => "chats",
            Self::Document => "documents",
            Self::Contact => "contacts",
            Self::Post => "posts",
            Self::Commit => "commits",
            Self::Issue => "issues",
            Self::PullRequest => "prs",
        }
    }
}

/// One raw item ready to land on disk.
pub struct RawItem<'a> {
    /// Stable upstream identifier (e.g. Gmail message id). Sanitised before use.
    pub uid: &'a str,
    /// Authoritative timestamp from the upstream item (ms since epoch). Drives
    /// the filename prefix so files sort chronologically.
    pub created_at_ms: i64,
    /// Markdown body to write.
    pub markdown: &'a str,
    /// Category subdir under the source.
    pub kind: RawKind,
}

/// Write a batch of raw items under `raw/<source_slug>/<kind>/`.
///
/// Returns the number of files written.
pub fn write_raw_items(
    content_root: &Path,
    source_id: &str,
    items: &[RawItem<'_>],
) -> Result<usize> {
    if items.is_empty() {
        return Ok(0);
    }
    let mut written = 0usize;
    for item in items {
        let dir = raw_kind_dir(content_root, source_id, item.kind);
        fs::create_dir_all(&dir).with_context(|| format!("create raw dir {}", dir.display()))?;
        let filename = build_filename(item.created_at_ms, item.uid);
        let path = dir.join(&filename);
        write_atomic(&path, item.markdown.as_bytes())
            .with_context(|| format!("write raw file {}", path.display()))?;
        written += 1;
    }
    Ok(written)
}

/// Resolve the per-source raw archive directory.
pub fn raw_source_dir(content_root: &Path, source_id: &str) -> PathBuf {
    let slug = slugify_source_id(source_id);
    content_root.join("raw").join(slug)
}

/// Resolve the on-disk directory for a single kind under a source.
pub fn raw_kind_dir(content_root: &Path, source_id: &str, kind: RawKind) -> PathBuf {
    raw_source_dir(content_root, source_id).join(kind.as_dir())
}

/// Forward-slash relative path of a raw file under `<content_root>/`.
pub fn raw_rel_path(source_id: &str, kind: RawKind, created_at_ms: i64, uid: &str) -> String {
    let slug = slugify_source_id(source_id);
    let filename = build_filename(created_at_ms, uid);
    format!("raw/{}/{}/{}", slug, kind.as_dir(), filename)
}

/// Build the `<ts>_<uid>.md` basename for a raw item.
///
/// `created_at_ms` is clamped to `0` (never negative) so a bad upstream
/// timestamp can't produce a leading `-` in the filename; `uid` is sanitised
/// via [`sanitize_uid`]. The same `(created_at_ms, uid)` pair always yields
/// the same filename, which is what makes [`write_raw_items`] idempotent on
/// re-sync.
fn build_filename(created_at_ms: i64, uid: &str) -> String {
    let ts = created_at_ms.max(0);
    let uid = sanitize_uid(uid);
    format!("{ts}_{uid}.md")
}

/// Replace path-illegal characters in the upstream uid before splicing it into
/// a filename.
/// Sanitize an upstream item identifier for use as a raw-archive filename.
pub fn sanitize_uid(uid: &str) -> String {
    let cleaned: String = uid
        .chars()
        .map(|c| match c {
            '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | ' ' => '-',
            other => other,
        })
        .collect();
    if cleaned.is_empty() {
        "unknown".into()
    } else {
        cleaned
    }
}

/// Write `bytes` to `path` via a sibling temp file (`.tmp_raw_<pid>_<nanos>.md`)
/// that is fsynced then renamed over the destination, fsyncing the parent
/// directory afterwards so the rename survives a crash.
///
/// Unlike [`super::atomic::write_if_new`], this always overwrites an existing
/// file at `path` — there is no existence check — relying on the caller
/// ([`write_raw_items`]) to only ever regenerate the same bytes for a given
/// `(source, uid, ts)` triple, which is what makes repeat writes idempotent
/// in effect rather than by construction.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("path has no parent: {}", path.display()))?;
    let tmp = parent.join(format!(
        ".tmp_raw_{}_{}.md",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let mut f = fs::File::create(&tmp).with_context(|| format!("create tmp {}", tmp.display()))?;
    f.write_all(bytes)
        .with_context(|| format!("write tmp {}", tmp.display()))?;
    f.sync_all()
        .with_context(|| format!("fsync tmp {}", tmp.display()))?;
    drop(f);
    fs::rename(&tmp, path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;
    if let Ok(dir_handle) = fs::File::open(parent) {
        let _ = dir_handle.sync_all();
    }
    Ok(())
}

/// Slug an account email like `stevent95@gmail.com` to
/// `stevent95-at-gmail-dot-com`.
///
/// Rules: lowercase; `@` → `-at-`; `.` → `-dot-`; other non-`[a-z0-9]` runs
/// collapse to a single `-`; trim leading/trailing `-`.
pub fn slug_account_email(email: &str) -> String {
    let lower = email.trim().to_lowercase();
    let mut out = String::with_capacity(lower.len() + 8);
    let mut last_dash = true;
    let chars = lower.chars().peekable();
    for ch in chars {
        match ch {
            '@' => {
                if !last_dash {
                    out.push('-');
                }
                out.push_str("at-");
                last_dash = true;
            }
            '.' => {
                if !last_dash {
                    out.push('-');
                }
                out.push_str("dot-");
                last_dash = true;
            }
            c if c.is_ascii_alphanumeric() => {
                out.push(c);
                last_dash = false;
            }
            _ => {
                if !last_dash {
                    out.push('-');
                    last_dash = true;
                }
            }
        }
    }
    let trimmed = out.trim_end_matches('-').trim_start_matches('-');
    if trimmed.is_empty() {
        "unknown".into()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
#[path = "raw_tests.rs"]
mod tests;
