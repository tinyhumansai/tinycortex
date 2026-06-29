//! Read and verify chunk and summary `.md` files from the content store.
//!
//! This port covers the self-contained read + integrity surface: front-matter
//! splitting, body SHA-256 verification, and untrusted-path resolution. The
//! Config/SQLite-aware high-level body readers (`read_chunk_body` /
//! `read_summary_body`, which resolve a chunk id → on-disk path through the
//! chunk store) are **deferred** along with the rest of the SQLite chunk-store
//! integration.

use std::path::{Component, Path, PathBuf};

use super::atomic::sha256_hex;
use super::compose::split_front_matter;

/// Resolve a DB-stored relative forward-slash path against `content_root`,
/// rejecting any traversal (`..`), absolute, or non-normal component.
///
/// `content_path` values are treated as **untrusted** at the read boundary: a
/// future ingest source or DB tamper could store `../../etc/passwd` and turn
/// this reader into an arbitrary file-disclosure primitive. We (1) reject any
/// `..`/absolute/prefix component before touching disk and (2) — when the
/// target exists — canonicalize and assert it stays under `content_root`.
pub fn resolve_within_content_root(content_root: &Path, rel_path: &str) -> anyhow::Result<PathBuf> {
    if Path::new(rel_path).is_absolute() {
        return Err(anyhow::anyhow!(
            "[content_store::read] rejected absolute path"
        ));
    }

    let mut abs = content_root.to_path_buf();
    for component in rel_path.split('/') {
        if component.is_empty() || component == "." {
            continue;
        }
        match Path::new(component).components().next() {
            Some(Component::Normal(_)) => abs.push(component),
            _ => {
                return Err(anyhow::anyhow!(
                    "[content_store::read] rejected unsafe path component"
                ));
            }
        }
    }

    if abs.exists() {
        let canon_root = content_root
            .canonicalize()
            .unwrap_or_else(|_| content_root.to_path_buf());
        let canon_abs = abs
            .canonicalize()
            .map_err(|e| anyhow::anyhow!("[content_store::read] canonicalize failed: {e}"))?;
        if !canon_abs.starts_with(&canon_root) {
            return Err(anyhow::anyhow!(
                "[content_store::read] resolved path escapes content_root"
            ));
        }
    }

    Ok(abs)
}

/// The result of reading a chunk file from disk.
#[derive(Debug, Clone)]
pub struct ChunkFileContents {
    /// The Markdown body (everything after the closing `---` of the front-matter).
    pub body: String,
    /// SHA-256 hex digest over the **body bytes** only.
    pub sha256: String,
}

/// Read a chunk file and return its body + SHA-256.
pub fn read_chunk_file(abs_path: &Path) -> anyhow::Result<ChunkFileContents> {
    let raw = std::fs::read(abs_path).map_err(|e| anyhow::anyhow!("read {:?}: {e}", abs_path))?;
    let content = std::str::from_utf8(&raw)
        .map_err(|e| anyhow::anyhow!("invalid UTF-8 in {:?}: {e}", abs_path))?;

    let (_fm, body) = split_front_matter(content)
        .ok_or_else(|| anyhow::anyhow!("no front-matter in {:?}", abs_path))?;

    let sha256 = sha256_hex(body.as_bytes());
    Ok(ChunkFileContents {
        body: body.to_string(),
        sha256,
    })
}

/// Verify that the body of a chunk file matches the expected SHA-256.
pub fn verify_chunk_file(abs_path: &Path, expected_sha256: &str) -> anyhow::Result<bool> {
    let contents = read_chunk_file(abs_path)?;
    Ok(contents.sha256 == expected_sha256)
}

/// The result of verifying a summary file on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyResult {
    /// The on-disk body SHA-256 matches the stored value.
    Ok,
    /// The file exists but the body SHA-256 does not match.
    Mismatch { actual: String },
    /// The file does not exist at the given path.
    Missing,
}

/// Read a summary file and return its body + SHA-256. The file format is
/// identical to a chunk file.
pub fn read_summary_file(abs_path: &Path) -> anyhow::Result<ChunkFileContents> {
    read_chunk_file(abs_path)
}

/// Verify a summary file's body SHA-256 without returning the body.
pub fn verify_summary_file(abs_path: &Path, expected_sha256: &str) -> anyhow::Result<VerifyResult> {
    if !abs_path.exists() {
        return Ok(VerifyResult::Missing);
    }
    let contents = read_summary_file(abs_path)?;
    if contents.sha256 == expected_sha256 {
        Ok(VerifyResult::Ok)
    } else {
        Ok(VerifyResult::Mismatch {
            actual: contents.sha256,
        })
    }
}

#[cfg(test)]
#[path = "read_tests.rs"]
mod tests;
