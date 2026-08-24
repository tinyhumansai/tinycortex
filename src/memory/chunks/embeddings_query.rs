//! Signature-aware embedding **read** queries for the chunk store.
//!
//! Split out of `embeddings` so that module stays a focused writer-side
//! accessor and re-embed tombstone store, and this one owns the per-signature
//! read paths. Every read matches *any* spelling of a vector space (see
//! [`super::signature::signature_variants`]) so a store that changed
//! conventions can still see its own prior vectors.
//!
//! No embeddings are computed here — embeddings.rs takes vectors on the write
//! side; this module only reads them back.

use super::connection::with_connection;
use super::embeddings::tree_active_signature;
use super::signature::{signature_in_clause, signature_variants};
use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension};
use std::collections::HashMap;

use crate::memory::config::MemoryConfig;
use crate::memory::store::content::read::{
    LEGACY_EMPTY_CONTENT_POINTER_REASON_PREFIX, LEGACY_NO_CONTENT_POINTER_REASON_PREFIX,
};

/// Fetch a chunk embedding for one provider/model/dimension signature — under
/// any of its spellings (see [`signature_variants`]).
///
/// Returns `Ok(None)` when no row exists for `(chunk_id, model_signature)` —
/// absence is not an error.
///
/// # Errors
/// Returns `Err` if the query fails, or if `embedding_from_blob` rejects
/// the stored blob (negative/zero-remainder-mismatched dim, or a blob length
/// not a multiple of 4 bytes — both indicate on-disk corruption of this row,
/// not a normal "no embedding" state).
pub fn get_chunk_embedding_for_signature(
    config: &MemoryConfig,
    chunk_id: &str,
    model_signature: &str,
) -> Result<Option<Vec<f32>>> {
    let variants = signature_variants(model_signature);
    with_connection(config, |conn| {
        let mut params: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(variants.len() + 1);
        params.push(&chunk_id as &dyn rusqlite::ToSql);
        for variant in &variants {
            params.push(variant as &dyn rusqlite::ToSql);
        }
        let row: Option<(Vec<u8>, i64)> = conn
            .query_row(
                &format!(
                    "SELECT vector, dim
                       FROM mem_tree_chunk_embeddings
                      WHERE chunk_id = ?1 AND model_signature {}",
                    signature_in_clause(variants.len(), 2)
                ),
                params.as_slice(),
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        match row {
            None => Ok(None),
            Some((bytes, dim)) => embedding_from_blob(&bytes, dim, "chunk embedding"),
        }
    })
}

/// Fetch a chunk's embedding for the active model signature (see
/// [`tree_active_signature`]). See [`get_chunk_embedding_for_signature`] for
/// the return/error contract.
pub fn get_chunk_embedding(config: &MemoryConfig, chunk_id: &str) -> Result<Option<Vec<f32>>> {
    let signature = tree_active_signature(config);
    get_chunk_embedding_for_signature(config, chunk_id, &signature)
}

/// Decode a little-endian `f32` vector `BLOB` back into `Vec<f32>`, validating
/// it against the DB's own recorded `dim` column. `label` only qualifies the
/// error message (e.g. `"chunk embedding"` vs. a future summary-embedding
/// caller).
///
/// Always returns `Ok(Some(_))` on success — the `Option` in the return type
/// exists purely so callers can `?`-propagate this directly from inside a
/// `match row { None => Ok(None), Some(..) => embedding_from_blob(..) }` arm
/// without an extra `.map`.
///
/// # Errors
/// Returns `Err` if `dim` is negative, if `bytes.len()` is not a multiple of
/// 4, or if the decoded float count does not equal `dim` — all three
/// indicate the stored row is internally inconsistent (corruption or a bug
/// upstream), not a normal "different embedding space" mismatch (that case is
/// handled by comparing signatures/dims *before* calling this, e.g. in
/// [`super::migrations::migrate_legacy_embeddings_to_sidecar`]).
fn embedding_from_blob(bytes: &[u8], dim: i64, label: &str) -> Result<Option<Vec<f32>>> {
    if dim < 0 {
        anyhow::bail!("{label} has negative dimension {dim}");
    }
    if !bytes.len().is_multiple_of(4) {
        anyhow::bail!("{label} blob length {} not a multiple of 4", bytes.len());
    }
    let floats: Vec<f32> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect();
    if floats.len() != dim as usize {
        anyhow::bail!(
            "{label} dimension mismatch: dim column says {dim}, blob contains {} floats",
            floats.len()
        );
    }
    Ok(Some(floats))
}

/// Whether any live chunk or summary lacks both an embedding and terminal
/// tombstone.
///
/// Coverage counts a vector written under *any* spelling of the signature: a
/// row that only looks uncovered because the convention changed is not work,
/// and re-embedding it would spend provider quota to reproduce a vector that
/// is already on disk.
pub fn has_uncovered_reembed_work(
    conn: &Connection,
    model_signature: &str,
) -> rusqlite::Result<bool> {
    let variants = signature_variants(model_signature);
    let sig_clause = signature_in_clause(variants.len(), 1);
    // Retryable skip reasons for legacy content-pointer rows; matched after the
    // signature-variant params (which occupy `?1..=?variants.len()`).
    let retry_idx_1 = variants.len() + 1;
    let retry_idx_2 = variants.len() + 2;
    let missing_pointer_retry_like =
        format!("body read failed: {LEGACY_NO_CONTENT_POINTER_REASON_PREFIX}%");
    // Legacy-only: older readers emitted this for present-but-empty content
    // pointers. Current readers filter those pointers before returning `Some`,
    // but existing skip rows must still become retriable after the fallback fix.
    let empty_pointer_retry_like =
        format!("body read failed: {LEGACY_EMPTY_CONTENT_POINTER_REASON_PREFIX}%");
    let mut params: Vec<&dyn rusqlite::ToSql> = variants
        .iter()
        .map(|variant| variant as &dyn rusqlite::ToSql)
        .collect();
    params.push(&missing_pointer_retry_like as &dyn rusqlite::ToSql);
    params.push(&empty_pointer_retry_like as &dyn rusqlite::ToSql);
    conn.query_row(
        &format!(
            "SELECT EXISTS(
                 SELECT 1 FROM mem_tree_chunks c
                  WHERE NOT EXISTS (SELECT 1 FROM mem_tree_chunk_embeddings e
                                     WHERE e.chunk_id = c.id AND e.model_signature {sig_clause})
                    AND NOT EXISTS (SELECT 1 FROM mem_tree_chunk_reembed_skipped sk
                                     WHERE sk.chunk_id = c.id AND sk.model_signature {sig_clause}
                                       AND sk.reason NOT LIKE ?{retry_idx_1}
                                       AND sk.reason NOT LIKE ?{retry_idx_2}))
               OR EXISTS(
                 SELECT 1 FROM mem_tree_summaries s
                  WHERE s.deleted = 0
                    AND NOT EXISTS (SELECT 1 FROM mem_tree_summary_embeddings e
                                     WHERE e.summary_id = s.id AND e.model_signature {sig_clause})
                    AND NOT EXISTS (SELECT 1 FROM mem_tree_summary_reembed_skipped sk
                                     WHERE sk.summary_id = s.id AND sk.model_signature {sig_clause}))"
        ),
        params.as_slice(),
        |row| row.get(0),
    )
}

/// Defensive cap for batched `IN (?,?,…)` reads, well below SQLite's
/// `SQLITE_MAX_VARIABLE_NUMBER` (32 766).
const MAX_EMBEDDING_BATCH: usize = 500;

/// Batched read of chunk embeddings under a single `model_signature`.
///
/// Returns a `HashMap<chunk_id, Vec<f32>>` containing only the chunks that have
/// a vector under `model_signature`. Missing chunks are simply absent (callers
/// treat that the same as a `None` from the single-row helper).
///
/// `chunk_ids` is split into windows of at most `MAX_EMBEDDING_BATCH` so a
/// single query never approaches SQLite's bound-parameter limit; each window
/// runs as its own `SELECT ... WHERE chunk_id IN (...)` inside the same
/// [`super::connection::with_connection`] call (not separately transacted —
/// reads only).
///
/// # Errors
/// Returns `Err` if `chunk_ids` is non-empty and any window's query
/// preparation, execution, or blob decoding (`embedding_from_blob`) fails.
/// Returns `Ok(HashMap::new())` immediately (no DB access) when `chunk_ids`
/// is empty.
pub fn get_chunk_embeddings_for_signature_batch(
    config: &MemoryConfig,
    chunk_ids: &[String],
    model_signature: &str,
) -> Result<HashMap<String, Vec<f32>>> {
    if chunk_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let variants = signature_variants(model_signature);
    with_connection(config, |conn| {
        let mut out: HashMap<String, Vec<f32>> = HashMap::with_capacity(chunk_ids.len());
        for window in chunk_ids.chunks(MAX_EMBEDDING_BATCH) {
            let placeholders = std::iter::repeat_n("?", window.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT chunk_id, vector, dim
                   FROM mem_tree_chunk_embeddings
                  WHERE chunk_id IN ({placeholders})
                    AND model_signature {sig_clause}",
                sig_clause = signature_in_clause(variants.len(), window.len() + 1),
            );
            let mut stmt = conn
                .prepare(&sql)
                .context("prepare get_chunk_embeddings_for_signature_batch")?;
            let mut params: Vec<&dyn rusqlite::ToSql> =
                Vec::with_capacity(window.len() + variants.len());
            for id in window {
                params.push(id as &dyn rusqlite::ToSql);
            }
            for variant in &variants {
                params.push(variant as &dyn rusqlite::ToSql);
            }
            let rows = stmt
                .query_map(params.as_slice(), |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                })
                .context("query get_chunk_embeddings_for_signature_batch")?;
            for row in rows {
                let (chunk_id, bytes, dim) = row?;
                if let Some(v) = embedding_from_blob(&bytes, dim, "chunk embedding")? {
                    out.insert(chunk_id, v);
                }
            }
        }
        Ok(out)
    })
}

/// Batched read of chunk embeddings under the **active** model signature. See
/// [`get_chunk_embeddings_for_signature_batch`] for the batching and error
/// contract.
pub fn get_chunk_embeddings_batch(
    config: &MemoryConfig,
    chunk_ids: &[String],
) -> Result<HashMap<String, Vec<f32>>> {
    let signature = tree_active_signature(config);
    get_chunk_embeddings_for_signature_batch(config, chunk_ids, &signature)
}
