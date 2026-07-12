//! Read and verify chunk and summary `.md` files from the content store.
//!
//! Includes high-level id-based readers that resolve SQLite pointers, hydrate
//! raw-backed chunks, and repair stale checksum tokens after external edits.

use std::path::{Component, Path, PathBuf};

use super::atomic::sha256_hex;
use super::compose::split_front_matter;
use crate::memory::chunks::{
    content_root, get_chunk_content_pointers, get_chunk_raw_refs, get_summary_content_pointers,
    update_chunk_content_sha256, update_summary_content_sha256, RawRef,
};
use crate::memory::config::MemoryConfig;

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
    Mismatch {
        /// The body SHA-256 actually found on disk.
        actual: String,
    },
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

/// Read a full chunk body by id. Raw archive references take precedence over
/// staged Markdown pointers. A drifted staged-file checksum is repaired
/// best-effort while the authoritative on-disk body is still returned.
pub fn read_chunk_body(config: &MemoryConfig, chunk_id: &str) -> anyhow::Result<String> {
    if let Some(refs) = get_chunk_raw_refs(config, chunk_id)? {
        if !refs.is_empty() {
            return read_chunk_body_from_raw(config, &refs);
        }
    }

    let (rel_path, expected_sha256) = get_chunk_content_pointers(config, chunk_id)?
        .ok_or_else(|| anyhow::anyhow!("no content pointer or raw refs for chunk {chunk_id}"))?;
    if rel_path.is_empty() {
        anyhow::bail!("empty content pointer and no raw refs for chunk {chunk_id}");
    }
    let abs_path = resolve_within_content_root(&content_root(config), &rel_path)?;
    let result = read_chunk_file(&abs_path)?;
    if result.sha256 != expected_sha256 {
        log::warn!("[memory:content] repairing stale chunk checksum token chunk_id={chunk_id}");
        if let Err(error) = update_chunk_content_sha256(config, chunk_id, &result.sha256) {
            log::warn!(
                "[memory:content] chunk checksum repair failed chunk_id={chunk_id}: {error}"
            );
        }
    }
    Ok(result.body)
}

fn read_chunk_body_from_raw(config: &MemoryConfig, refs: &[RawRef]) -> anyhow::Result<String> {
    let root = content_root(config);
    let mut parts = Vec::with_capacity(refs.len());
    for reference in refs {
        let path = match resolve_within_content_root(&root, &reference.path) {
            Ok(path) => path,
            Err(error) => {
                log::warn!("[memory:content] rejected unsafe raw reference: {error}");
                continue;
            }
        };
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) => {
                log::warn!("[memory:content] raw reference read failed: {error}");
                continue;
            }
        };
        let start = reference.start.min(bytes.len());
        let end = reference.end.unwrap_or(bytes.len()).min(bytes.len());
        if end <= start {
            continue;
        }
        match std::str::from_utf8(&bytes[start..end]) {
            Ok(value) => parts.push(value.to_owned()),
            Err(error) => log::warn!("[memory:content] raw reference is not UTF-8: {error}"),
        }
    }
    Ok(parts.join("\n\n"))
}

/// Read a full summary body by id and repair a stale checksum token
/// best-effort when an external editor changed the staged file.
pub fn read_summary_body(config: &MemoryConfig, summary_id: &str) -> anyhow::Result<String> {
    let (rel_path, expected_sha256) = get_summary_content_pointers(config, summary_id)?
        .ok_or_else(|| anyhow::anyhow!("no content pointer for summary {summary_id}"))?;
    let abs_path = resolve_within_content_root(&content_root(config), &rel_path)?;
    let result = read_summary_file(&abs_path)?;
    if result.sha256 != expected_sha256 {
        log::warn!(
            "[memory:content] repairing stale summary checksum token summary_id={summary_id}"
        );
        if let Err(error) = update_summary_content_sha256(config, summary_id, &result.sha256) {
            log::warn!(
                "[memory:content] summary checksum repair failed summary_id={summary_id}: {error}"
            );
        }
    }
    Ok(result.body)
}

#[cfg(test)]
#[path = "read_tests.rs"]
mod tests;
