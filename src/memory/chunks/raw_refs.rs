//! Raw-archive pointers and content-pointer accessors for chunk/summary rows.
//!
//! [`RawRef`] lets ingest pipelines mirror full message bodies to on-disk
//! archives under `<content_root>/raw/` while storing only a ≤500-char preview
//! in the SQLite `content` column. Retrieval reads the archive directly instead
//! of going through the SQL preview path.

use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension, Transaction};

use super::connection::with_connection;
use crate::memory::config::MemoryConfig;

/// One pointer into the raw archive. A chunk's body is reconstructed by reading
/// each [`RawRef`] in order and joining with `"\n\n"`.
///
/// `start` / `end` are byte offsets into the raw `.md` file. `end = None` means
/// "read to end of file". Both default to "the whole file" (`start = 0`,
/// `end = None`).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct RawRef {
    /// Forward-slash relative path under `<content_root>/`.
    pub path: String,
    /// Byte offset where this chunk's slice begins.
    #[serde(default)]
    pub start: usize,
    /// Byte offset where this chunk's slice ends (`None` = end of file).
    #[serde(default)]
    pub end: Option<usize>,
}

/// Stash a list of [`RawRef`] entries on a chunk row. Replaces any previous
/// value.
pub fn set_chunk_raw_refs(config: &MemoryConfig, chunk_id: &str, refs: &[RawRef]) -> Result<()> {
    let json = serde_json::to_string(refs).context("serialize raw_refs")?;
    with_connection(config, |conn| {
        conn.execute(
            "UPDATE mem_tree_chunks SET raw_refs_json = ?1 WHERE id = ?2",
            params![json, chunk_id],
        )?;
        Ok(())
    })
}

/// Stash raw archive pointers on a chunk row inside a caller-owned transaction.
pub fn set_chunk_raw_refs_tx(tx: &Transaction<'_>, chunk_id: &str, refs: &[RawRef]) -> Result<()> {
    let json = serde_json::to_string(refs).context("serialize raw_refs")?;
    tx.execute(
        "UPDATE mem_tree_chunks SET raw_refs_json = ?1 WHERE id = ?2",
        params![json, chunk_id],
    )?;
    Ok(())
}

/// Return the raw-archive pointers stored in SQLite for `chunk_id`, or `None`
/// if no `raw_refs_json` was recorded.
pub fn get_chunk_raw_refs(config: &MemoryConfig, chunk_id: &str) -> Result<Option<Vec<RawRef>>> {
    with_connection(config, |conn| {
        let row = conn
            .query_row(
                "SELECT raw_refs_json FROM mem_tree_chunks WHERE id = ?1",
                params![chunk_id],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten();
        match row {
            Some(json) if !json.is_empty() => {
                let refs: Vec<RawRef> =
                    serde_json::from_str(&json).context("deserialize raw_refs_json")?;
                Ok(Some(refs))
            }
            _ => Ok(None),
        }
    })
}

/// Collect every raw-archive path referenced by ANY chunk row whose
/// `raw_refs_json` is set, restricted to paths under `rel_prefix`. Rust-side
/// prefix filter so `_` / `%` in slugs are treated literally.
pub fn list_chunk_raw_ref_paths_with_prefix(
    config: &MemoryConfig,
    rel_prefix: &str,
) -> Result<std::collections::HashSet<String>> {
    with_connection(config, |conn| {
        let mut stmt = conn.prepare(
            "SELECT raw_refs_json FROM mem_tree_chunks \
              WHERE raw_refs_json IS NOT NULL AND raw_refs_json != ''",
        )?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out: std::collections::HashSet<String> = std::collections::HashSet::new();
        for row in rows {
            let json = row?;
            match serde_json::from_str::<Vec<RawRef>>(&json) {
                Ok(refs) => {
                    for raw_ref in refs {
                        if raw_ref.path.starts_with(rel_prefix) {
                            out.insert(raw_ref.path);
                        }
                    }
                }
                // Tolerate individually-corrupt rows: skip rather than failing
                // the whole coverage scan.
                Err(_) => {}
            }
        }
        Ok(out)
    })
}

/// Return both `content_path` and `content_sha256` stored in SQLite for `chunk_id`.
pub fn get_chunk_content_pointers(
    config: &MemoryConfig,
    chunk_id: &str,
) -> Result<Option<(String, String)>> {
    with_connection(config, |conn| {
        let row = conn
            .query_row(
                "SELECT content_path, content_sha256 FROM mem_tree_chunks WHERE id = ?1",
                params![chunk_id],
                |r| {
                    let path: Option<String> = r.get(0)?;
                    let sha: Option<String> = r.get(1)?;
                    Ok((path, sha))
                },
            )
            .optional()?;
        Ok(row.and_then(|(p, s)| p.zip(s)))
    })
}

/// Return the `content_path` stored in SQLite for `chunk_id`, if any.
pub fn get_chunk_content_path(config: &MemoryConfig, chunk_id: &str) -> Result<Option<String>> {
    with_connection(config, |conn| {
        let row = conn
            .query_row(
                "SELECT content_path FROM mem_tree_chunks WHERE id = ?1",
                params![chunk_id],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten();
        Ok(row)
    })
}

/// Return both `content_path` and `content_sha256` stored in SQLite for `summary_id`.
pub fn get_summary_content_pointers(
    config: &MemoryConfig,
    summary_id: &str,
) -> Result<Option<(String, String)>> {
    with_connection(config, |conn| {
        let row = conn
            .query_row(
                "SELECT content_path, content_sha256 FROM mem_tree_summaries WHERE id = ?1",
                params![summary_id],
                |r| {
                    let path: Option<String> = r.get(0)?;
                    let sha: Option<String> = r.get(1)?;
                    Ok((path, sha))
                },
            )
            .optional()?;
        Ok(row.and_then(|(p, s)| p.zip(s)))
    })
}

/// List all summary rows that have a non-NULL `content_path`.
pub fn list_summaries_with_content_path(
    config: &MemoryConfig,
) -> Result<Vec<(String, String, String)>> {
    with_connection(config, |conn| {
        let mut stmt = conn.prepare(
            "SELECT id, content_path, content_sha256
               FROM mem_tree_summaries
              WHERE content_path IS NOT NULL AND content_sha256 IS NOT NULL
                AND deleted = 0",
        )?;
        let rows = stmt
            .query_map([], |r| {
                let id: String = r.get(0)?;
                let path: String = r.get(1)?;
                let sha: String = r.get(2)?;
                Ok((id, path, sha))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("Failed to list summaries with content_path")?;
        Ok(rows)
    })
}
