//! Shared filesystem primitives for the memory engine.
//!
//! `atomic_write` is the single crash-safe write path used by the on-disk
//! stores (goals list, time-tree nodes, staged summaries). It writes to a
//! hidden same-directory temp file, fsyncs it, then renames it over the
//! destination. Because POSIX `rename(2)` is atomic, a crash or a concurrent
//! reader always observes either the previous file contents or the fully
//! written new contents — never a truncated or empty file. On any failure the
//! temp file is removed so no litter accumulates.
//!
//! This mirrors the temp-file + rename pattern already used by the source
//! registry (`sources::registry::SourceRegistry::atomic_write`); the stores
//! share this helper rather than re-implementing it per module.

use std::io;
use std::path::Path;

/// Sanitize one machine identifier into a path component while preserving
/// collision resistance. Safe, non-empty inputs retain their spelling;
/// transformed or empty inputs receive a short digest of the exact raw value.
pub(crate) fn sanitize_component_with_digest(raw: &str, allowed: impl Fn(char) -> bool) -> String {
    use sha2::{Digest, Sha256};

    let sanitized = raw
        .chars()
        .map(|character| if allowed(character) { character } else { '_' })
        .collect::<String>();
    if sanitized == raw && !sanitized.is_empty() {
        return sanitized;
    }
    let digest = Sha256::digest(raw.as_bytes());
    let suffix = digest[..6]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("{sanitized}-{suffix}")
}

/// Atomically write `bytes` to `path`, replacing any existing file.
///
/// The bytes are written to a hidden same-directory temp file and fsynced, then
/// renamed over `path`; on Unix the parent directory is fsynced afterwards so
/// the rename survives a crash. The destination's parent directory must already
/// exist (callers that need it created should `create_dir_all` first). If any
/// step fails the temp file is cleaned up and the original destination is left
/// untouched.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;

    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let filename = path.file_name().and_then(|n| n.to_str()).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("path has no file name: {}", path.display()),
        )
    })?;
    let tmp_path = parent.join(format!(
        ".{filename}.tmp-{}",
        uuid::Uuid::new_v4().as_simple()
    ));

    let result = (|| -> io::Result<()> {
        {
            let mut file = std::fs::File::create(&tmp_path)?;
            file.write_all(bytes)?;
            file.sync_all()?;
        }
        std::fs::rename(&tmp_path, path)?;
        // fsync the parent directory so the rename itself is durable.
        #[cfg(unix)]
        if let Ok(dir) = std::fs::File::open(parent) {
            let _ = dir.sync_all();
        }
        Ok(())
    })();

    if result.is_err() {
        let _ = std::fs::remove_file(&tmp_path);
    }
    result
}

#[cfg(test)]
#[path = "fsutil_tests.rs"]
mod tests;
