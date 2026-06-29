//! Per-(chunk, embedding-model) vector accessors and re-embed tombstones.
//!
//! Embeddings are stored in the `mem_tree_chunk_embeddings` sidecar table keyed
//! by `(chunk_id, model_signature)` so multiple vector spaces can coexist. This
//! module is pure storage: it does not compute embeddings (that backend is not
//! ported here) — callers pass vectors in.

use super::connection::with_connection;
use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension};
use std::collections::HashMap;

use crate::memory::config::MemoryConfig;

/// The active embedding vector dimension for `config`. Drives the legacy
/// migration's dim-match decision.
pub(crate) fn active_embedding_dims(config: &MemoryConfig) -> usize {
    config.embedding.dim
}

/// Resolve the active embedding signature — the canonical key every per-model
/// sidecar read/write is scoped by. Derived from the configured model + dim so
/// a provider/model/dimension switch becomes a query-time filter rather than a
/// destructive rewrite.
pub fn tree_active_signature(config: &MemoryConfig) -> String {
    format!("{}@{}", config.embedding.model, config.embedding.dim)
}

/// Store a chunk's embedding under the active model signature.
pub fn set_chunk_embedding(config: &MemoryConfig, chunk_id: &str, embedding: &[f32]) -> Result<()> {
    let signature = tree_active_signature(config);
    set_chunk_embedding_for_signature(config, chunk_id, &signature, embedding)
}

/// Core upsert into `mem_tree_chunk_embeddings` over an arbitrary `&Connection`.
/// `rusqlite::Transaction` derefs to `Connection`, so an in-tx caller passes
/// `&tx` and the sidecar row commits atomically with the surrounding work.
fn upsert_chunk_embedding_conn(
    conn: &Connection,
    chunk_id: &str,
    model_signature: &str,
    embedding: &[f32],
) -> Result<()> {
    let bytes = embedding_to_blob(embedding);
    let dim = i64::try_from(embedding.len()).context("embedding dimension does not fit i64")?;
    let created_at = Utc::now().timestamp_millis() as f64 / 1000.0;
    conn.execute(
        "INSERT INTO mem_tree_chunk_embeddings
             (chunk_id, model_signature, vector, dim, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(chunk_id, model_signature) DO UPDATE SET
                vector = excluded.vector,
                dim = excluded.dim,
                created_at = excluded.created_at",
        rusqlite::params![chunk_id, model_signature, bytes, dim, created_at],
    )?;
    Ok(())
}

/// Core upsert into `mem_tree_summary_embeddings` over an arbitrary
/// `&Connection`. Used only by the legacy→sidecar migration here.
fn upsert_summary_embedding_conn(
    conn: &Connection,
    summary_id: &str,
    model_signature: &str,
    embedding: &[f32],
) -> Result<()> {
    let bytes = embedding_to_blob(embedding);
    let dim = i64::try_from(embedding.len()).context("embedding dimension does not fit i64")?;
    let created_at = Utc::now().timestamp_millis() as f64 / 1000.0;
    conn.execute(
        "INSERT INTO mem_tree_summary_embeddings
             (summary_id, model_signature, vector, dim, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(summary_id, model_signature) DO UPDATE SET
                vector = excluded.vector,
                dim = excluded.dim,
                created_at = excluded.created_at",
        rusqlite::params![summary_id, model_signature, bytes, dim, created_at],
    )?;
    Ok(())
}

/// Store a chunk embedding for a specific provider/model/dimension signature.
pub fn set_chunk_embedding_for_signature(
    config: &MemoryConfig,
    chunk_id: &str,
    model_signature: &str,
    embedding: &[f32],
) -> Result<()> {
    with_connection(config, |conn| {
        upsert_chunk_embedding_conn(conn, chunk_id, model_signature, embedding)
    })
}

/// Transaction-scoped variant of [`set_chunk_embedding_for_signature`].
pub(crate) fn set_chunk_embedding_for_signature_tx(
    tx: &rusqlite::Transaction<'_>,
    chunk_id: &str,
    model_signature: &str,
    embedding: &[f32],
) -> Result<()> {
    upsert_chunk_embedding_conn(tx, chunk_id, model_signature, embedding)
}

/// Transaction-scoped summary embedding upsert (used by the legacy migration).
pub(crate) fn set_summary_embedding_for_signature_tx(
    tx: &rusqlite::Transaction<'_>,
    summary_id: &str,
    model_signature: &str,
    embedding: &[f32],
) -> Result<()> {
    upsert_summary_embedding_conn(tx, summary_id, model_signature, embedding)
}

/// Persistently record that `(chunk_id, signature)` cannot be re-embedded so a
/// future backfill worklist can exclude it instead of looping on the same row.
pub fn mark_chunk_reembed_skipped(
    config: &MemoryConfig,
    chunk_id: &str,
    model_signature: &str,
    reason: &str,
) -> Result<()> {
    let chunk_id = validate_reembed_skip_key("chunk_id", chunk_id)?;
    let model_signature = validate_reembed_skip_key("model_signature", model_signature)?;
    with_connection(config, |conn| {
        let now_ms = Utc::now().timestamp_millis();
        conn.execute(
            "INSERT INTO mem_tree_chunk_reembed_skipped
                 (chunk_id, model_signature, reason, skipped_at_ms)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(chunk_id, model_signature) DO UPDATE SET
                    reason = excluded.reason,
                    skipped_at_ms = excluded.skipped_at_ms",
            rusqlite::params![chunk_id, model_signature, reason, now_ms],
        )?;
        Ok(())
    })
}

/// Remove a single chunk tombstone so re-embed backfill can retry the row.
/// Idempotent.
pub fn clear_chunk_reembed_skipped(
    config: &MemoryConfig,
    chunk_id: &str,
    model_signature: &str,
) -> Result<()> {
    let chunk_id = validate_reembed_skip_key("chunk_id", chunk_id)?;
    let model_signature = validate_reembed_skip_key("model_signature", model_signature)?;
    with_connection(config, |conn| {
        conn.execute(
            "DELETE FROM mem_tree_chunk_reembed_skipped
              WHERE chunk_id = ?1 AND model_signature = ?2",
            rusqlite::params![chunk_id, model_signature],
        )?;
        Ok(())
    })
}

/// Clear all chunk and summary tombstones for a model signature. Returns the
/// total number of rows removed across both tombstone tables. Idempotent.
pub fn clear_reembed_skipped_for_signature(
    config: &MemoryConfig,
    model_signature: &str,
) -> Result<usize> {
    let model_signature = validate_reembed_skip_key("model_signature", model_signature)?;
    with_connection(config, |conn| {
        let chunk_deleted = conn.execute(
            "DELETE FROM mem_tree_chunk_reembed_skipped WHERE model_signature = ?1",
            rusqlite::params![model_signature],
        )?;
        let summary_deleted = conn.execute(
            "DELETE FROM mem_tree_summary_reembed_skipped WHERE model_signature = ?1",
            rusqlite::params![model_signature],
        )?;
        Ok(chunk_deleted + summary_deleted)
    })
}

/// Bounds attacker-controlled ids/signatures passed to reembed-skipped admin
/// helpers. Rejects NUL bytes so SQLite bindings cannot be truncated.
pub(crate) const REEMBED_SKIP_KEY_MAX_LEN: usize = 2048;

pub(crate) fn validate_reembed_skip_key<'a>(label: &str, value: &'a str) -> Result<&'a str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        anyhow::bail!("{label} must be non-empty");
    }
    if trimmed.len() > REEMBED_SKIP_KEY_MAX_LEN {
        anyhow::bail!("{label} exceeds maximum length ({REEMBED_SKIP_KEY_MAX_LEN})");
    }
    if trimmed.as_bytes().contains(&0) {
        anyhow::bail!("{label} must not contain NUL bytes");
    }
    Ok(trimmed)
}

/// Fetch a chunk embedding for exactly one provider/model/dimension signature.
pub fn get_chunk_embedding_for_signature(
    config: &MemoryConfig,
    chunk_id: &str,
    model_signature: &str,
) -> Result<Option<Vec<f32>>> {
    with_connection(config, |conn| {
        let row: Option<(Vec<u8>, i64)> = conn
            .query_row(
                "SELECT vector, dim
                   FROM mem_tree_chunk_embeddings
                  WHERE chunk_id = ?1 AND model_signature = ?2",
                rusqlite::params![chunk_id, model_signature],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        match row {
            None => Ok(None),
            Some((bytes, dim)) => embedding_from_blob(&bytes, dim, "chunk embedding"),
        }
    })
}

/// Fetch a chunk's embedding for the active model signature.
pub fn get_chunk_embedding(config: &MemoryConfig, chunk_id: &str) -> Result<Option<Vec<f32>>> {
    let signature = tree_active_signature(config);
    get_chunk_embedding_for_signature(config, chunk_id, &signature)
}

pub(crate) fn embedding_to_blob(embedding: &[f32]) -> Vec<u8> {
    embedding.iter().flat_map(|f| f.to_le_bytes()).collect()
}

fn embedding_from_blob(bytes: &[u8], dim: i64, label: &str) -> Result<Option<Vec<f32>>> {
    if dim < 0 {
        anyhow::bail!("{label} has negative dimension {dim}");
    }
    if !bytes.len().is_multiple_of(4) {
        anyhow::bail!("{label} blob length {} not a multiple of 4", bytes.len());
    }
    let floats: Vec<f32> = bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    if floats.len() != dim as usize {
        anyhow::bail!(
            "{label} dimension mismatch: dim column says {dim}, blob contains {} floats",
            floats.len()
        );
    }
    Ok(Some(floats))
}

/// Defensive cap for batched `IN (?,?,…)` reads, well below SQLite's
/// `SQLITE_MAX_VARIABLE_NUMBER` (32 766).
const MAX_EMBEDDING_BATCH: usize = 500;

/// Batched read of chunk embeddings under a single `model_signature`.
///
/// Returns a `HashMap<chunk_id, Vec<f32>>` containing only the chunks that have
/// a vector under `model_signature`. Missing chunks are simply absent (callers
/// treat that the same as a `None` from the single-row helper).
pub fn get_chunk_embeddings_for_signature_batch(
    config: &MemoryConfig,
    chunk_ids: &[String],
    model_signature: &str,
) -> Result<HashMap<String, Vec<f32>>> {
    if chunk_ids.is_empty() {
        return Ok(HashMap::new());
    }
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
                    AND model_signature = ?{sig_idx}",
                sig_idx = window.len() + 1,
            );
            let mut stmt = conn
                .prepare(&sql)
                .context("prepare get_chunk_embeddings_for_signature_batch")?;
            let mut params: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(window.len() + 1);
            for id in window {
                params.push(id as &dyn rusqlite::ToSql);
            }
            params.push(&model_signature as &dyn rusqlite::ToSql);
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

/// Batched read of chunk embeddings under the **active** model signature.
pub fn get_chunk_embeddings_batch(
    config: &MemoryConfig,
    chunk_ids: &[String],
) -> Result<HashMap<String, Vec<f32>>> {
    let signature = tree_active_signature(config);
    get_chunk_embeddings_for_signature_batch(config, chunk_ids, &signature)
}
