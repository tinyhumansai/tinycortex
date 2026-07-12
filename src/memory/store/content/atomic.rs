//! Atomic content-file writes via tempfile + fsync + rename.
//!
//! Each chunk body is written to `<parent>/.tmp_<uuid>.md`, then renamed to its
//! final path. The rename is atomic on any POSIX filesystem.
//!
//! **Immutability contract**: once a file exists at `abs_path`, it is never
//! overwritten by [`write_if_new`]. Callers detect "already exists" and skip.

use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::Path;

use super::compose::{compose_summary_md, split_front_matter, SummaryComposeInput};
use super::paths::{summary_rel_path_with_layout, SummaryDiskLayout};

/// Write `bytes` atomically to `abs_path` if the file does not already exist.
///
/// Returns `Ok(true)` when newly written, `Ok(false)` when it already existed.
pub fn write_if_new(abs_path: &Path, bytes: &[u8]) -> anyhow::Result<bool> {
    if abs_path.exists() {
        return Ok(false);
    }

    let parent = abs_path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)
        .map_err(|e| anyhow::anyhow!("create_dir_all {:?}: {e}", parent))?;

    let tmp_name = format!(".tmp_{}.md", uuid_v4_hex());
    let tmp_path = parent.join(&tmp_name);

    {
        let mut f = std::fs::File::create(&tmp_path)
            .map_err(|e| anyhow::anyhow!("create tempfile {:?}: {e}", tmp_path))?;
        f.write_all(bytes)
            .map_err(|e| anyhow::anyhow!("write tempfile {:?}: {e}", tmp_path))?;
        f.sync_all()
            .map_err(|e| anyhow::anyhow!("fsync tempfile {:?}: {e}", tmp_path))?;
    }

    match std::fs::rename(&tmp_path, abs_path) {
        Ok(()) => {
            // fsync the parent directory so the rename is durable across a crash.
            #[cfg(unix)]
            if let Some(parent) = abs_path.parent() {
                if let Ok(dir) = std::fs::File::open(parent) {
                    let _ = dir.sync_all();
                }
            }
            Ok(true)
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp_path);
            if abs_path.exists() {
                // Lost the race — another writer created the file first.
                Ok(false)
            } else {
                Err(anyhow::anyhow!(
                    "rename {:?} -> {:?}: {e}",
                    tmp_path,
                    abs_path
                ))
            }
        }
    }
}

/// Ensure `abs_path` contains `full_bytes` with the expected body digest.
///
/// Matching files are left untouched. Missing, malformed, or stale files are
/// replaced atomically so callers can safely persist `body_sha256` alongside
/// the returned content path.
pub fn write_or_replace_body(
    abs_path: &Path,
    full_bytes: &[u8],
    body_sha256: &str,
) -> anyhow::Result<()> {
    if abs_path.exists() {
        let disk_sha = read_body_sha256(abs_path).unwrap_or_default();
        if disk_sha == body_sha256 {
            return Ok(());
        }
    }

    if let Some(parent) = abs_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow::anyhow!("create content parent {:?}: {e}", parent))?;
    }

    crate::memory::fsutil::atomic_write(abs_path, full_bytes)
        .map_err(|e| anyhow::anyhow!("atomic content write {:?}: {e}", abs_path))
}

/// A summary that has been written to disk and is ready for SQLite upsert.
#[derive(Debug, Clone)]
pub struct StagedSummary {
    /// Identifier of the summary that was staged.
    pub summary_id: String,
    /// Relative content path (forward-slash).
    pub content_path: String,
    /// SHA-256 hex digest over the **body bytes** only (front-matter excluded).
    pub content_sha256: String,
}

/// Write a summary `.md` file to disk and return a [`StagedSummary`].
///
/// If the file already exists with the same body SHA-256, the existing
/// `StagedSummary` is returned without rewriting.
pub fn stage_summary(
    content_root: &Path,
    input: &SummaryComposeInput<'_>,
    scope_slug: &str,
) -> anyhow::Result<StagedSummary> {
    stage_summary_with_layout(content_root, input, scope_slug, SummaryDiskLayout::Standard)
}

/// Layout-aware variant of [`stage_summary`].
pub fn stage_summary_with_layout(
    content_root: &Path,
    input: &SummaryComposeInput<'_>,
    scope_slug: &str,
    layout: SummaryDiskLayout<'_>,
) -> anyhow::Result<StagedSummary> {
    let rel_path = summary_rel_path_with_layout(
        input.tree_kind,
        scope_slug,
        input.level,
        input.summary_id,
        layout,
    );
    let abs_path = {
        let mut abs = content_root.to_path_buf();
        for component in rel_path.split('/') {
            abs.push(component);
        }
        abs
    };

    let composed = compose_summary_md(input);
    let body_bytes = composed.body.as_bytes();
    let sha256 = sha256_hex(body_bytes);

    let full_bytes = composed.full.as_bytes();

    write_or_replace_body(&abs_path, full_bytes, &sha256)?;

    Ok(StagedSummary {
        summary_id: input.summary_id.to_string(),
        content_path: rel_path,
        content_sha256: sha256,
    })
}

/// Read a `.md` file, split off the YAML front-matter, and return the SHA-256
/// hex digest of the **body bytes only**.
fn read_body_sha256(path: &Path) -> anyhow::Result<String> {
    let raw = std::fs::read(path)?;
    let content = std::str::from_utf8(&raw)?;
    let (_fm, body) = split_front_matter(content)
        .ok_or_else(|| anyhow::anyhow!("no front-matter in {:?}", path))?;
    Ok(sha256_hex(body.as_bytes()))
}

/// Compute the SHA-256 hex digest of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write;
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// Tiny deterministic-ish hex string for temp file names.
fn uuid_v4_hex() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!(
        "{:08x}{:016x}",
        t,
        n.wrapping_mul(0x9e37_79b9_7f4a_7c15).wrapping_add(t as u64)
    )
}

#[cfg(test)]
#[path = "atomic_tests.rs"]
mod tests;
