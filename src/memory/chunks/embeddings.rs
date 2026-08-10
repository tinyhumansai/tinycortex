//! Per-(chunk, embedding-model) vector accessors and re-embed tombstones.
//!
//! Embeddings are stored in the `mem_tree_chunk_embeddings` sidecar table keyed
//! by `(chunk_id, model_signature)` so multiple vector spaces can coexist. This
//! module is pure storage: it does not compute embeddings (that backend is not
//! ported here) — callers pass vectors in. The signature-aware *read* side
//! lives in the [`super::embeddings_query`] sibling, which this module's
//! re-exports also expose at the `chunks` level.

use super::connection::with_connection;
use super::signature::format_signature;
use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::Connection;

use crate::memory::config::MemoryConfig;

/// The active embedding vector dimension for `config`. Drives the legacy
/// migration's dim-match decision.
pub(crate) fn active_embedding_dims(config: &MemoryConfig) -> usize {
    config.embedding.dim
}

/// Resolve the active embedding signature — the canonical key every per-model
/// sidecar read/write is scoped by. Derived from the configured provider, model
/// and dim so a provider/model/dimension switch becomes a query-time filter
/// rather than a destructive rewrite.
///
/// This is the canonical `provider=…;model=…;dims=…` spelling, shared with the
/// namespace store. Rows written under the tree's older `{model}@{dims}`
/// spelling are still found, because per-signature reads match every variant
/// (see [`super::signature::signature_variants`]) rather than one exact string.
pub fn tree_active_signature(config: &MemoryConfig) -> String {
    format_signature(
        &config.embedding.provider,
        &config.embedding.model,
        config.embedding.dim,
    )
}

/// Store a chunk's embedding under the active model signature (see
/// [`tree_active_signature`]).
///
/// Upserts: calling this again for the same `chunk_id` (under the same active
/// signature) replaces the stored vector rather than erroring or duplicating.
///
/// # Errors
/// Returns `Err` if `embedding.len()` does not fit in an `i64` (practically
/// unreachable), or if the underlying `INSERT ... ON CONFLICT` fails.
pub fn set_chunk_embedding(config: &MemoryConfig, chunk_id: &str, embedding: &[f32]) -> Result<()> {
    let signature = tree_active_signature(config);
    set_chunk_embedding_for_signature(config, chunk_id, &signature, embedding)
}

/// Core upsert into `mem_tree_chunk_embeddings` over an arbitrary `&Connection`.
/// `rusqlite::Transaction` derefs to `Connection`, so an in-tx caller passes
/// `&tx` and the sidecar row commits atomically with the surrounding work.
///
/// # Errors
/// Returns `Err` if `embedding.len()` overflows `i64` (practically
/// unreachable) or the `INSERT ... ON CONFLICT` statement fails.
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
    conn.execute(
        "DELETE FROM mem_tree_chunk_reembed_skipped
          WHERE chunk_id = ?1 AND model_signature = ?2",
        rusqlite::params![chunk_id, model_signature],
    )?;
    Ok(())
}

/// Core upsert into `mem_tree_summary_embeddings` over an arbitrary
/// `&Connection`. Used only by the legacy→sidecar migration here.
///
/// # Errors
/// See [`upsert_chunk_embedding_conn`] (identical failure modes).
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
///
/// Upserts by `(chunk_id, model_signature)` — safe to call repeatedly for the
/// same key; the latest call wins.
///
/// # Errors
/// See [`set_chunk_embedding`].
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

/// Transaction-scoped variant of [`set_chunk_embedding_for_signature`]; the
/// caller commits (or rolls back) `tx`.
///
/// # Errors
/// Uses the same validation as the connection-scoped upsert helper.
pub fn set_chunk_embedding_for_signature_tx(
    tx: &rusqlite::Transaction<'_>,
    chunk_id: &str,
    model_signature: &str,
    embedding: &[f32],
) -> Result<()> {
    upsert_chunk_embedding_conn(tx, chunk_id, model_signature, embedding)
}

/// Transaction-scoped summary embedding upsert (used by the legacy migration).
///
/// # Errors
/// Uses the same validation as the connection-scoped upsert helper.
pub fn set_summary_embedding_for_signature_tx(
    tx: &rusqlite::Transaction<'_>,
    summary_id: &str,
    model_signature: &str,
    embedding: &[f32],
) -> Result<()> {
    upsert_summary_embedding_conn(tx, summary_id, model_signature, embedding)
}

/// Persistently record that `(chunk_id, signature)` cannot be re-embedded so a
/// future backfill worklist can exclude it instead of looping on the same row.
///
/// Upserts by `(chunk_id, model_signature)`: a repeat call for the same key
/// overwrites `reason` and `skipped_at_ms` rather than erroring.
///
/// # Errors
/// Returns `Err` if `chunk_id` or `model_signature` fails
/// `validate_reembed_skip_key` (empty after trimming, over
/// `REEMBED_SKIP_KEY_MAX_LEN`, or contains a NUL byte), or if the `INSERT`
/// fails.
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
/// Idempotent — deleting a non-existent tombstone is a successful no-op.
///
/// # Errors
/// Returns `Err` if key validation fails (see
/// [`mark_chunk_reembed_skipped`]) or the `DELETE` fails.
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

pub fn mark_summary_reembed_skipped(
    config: &MemoryConfig,
    summary_id: &str,
    model_signature: &str,
    reason: &str,
) -> Result<()> {
    let summary_id = validate_reembed_skip_key("summary_id", summary_id)?;
    let model_signature = validate_reembed_skip_key("model_signature", model_signature)?;
    with_connection(config, |conn| {
        conn.execute(
            "INSERT INTO mem_tree_summary_reembed_skipped
             (summary_id, model_signature, reason, skipped_at_ms)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(summary_id, model_signature) DO UPDATE SET
                reason = excluded.reason, skipped_at_ms = excluded.skipped_at_ms",
            rusqlite::params![
                summary_id,
                model_signature,
                reason,
                Utc::now().timestamp_millis()
            ],
        )?;
        Ok(())
    })
}

pub fn clear_summary_reembed_skipped(
    config: &MemoryConfig,
    summary_id: &str,
    model_signature: &str,
) -> Result<()> {
    let summary_id = validate_reembed_skip_key("summary_id", summary_id)?;
    let model_signature = validate_reembed_skip_key("model_signature", model_signature)?;
    with_connection(config, |conn| {
        conn.execute(
            "DELETE FROM mem_tree_summary_reembed_skipped
             WHERE summary_id = ?1 AND model_signature = ?2",
            rusqlite::params![summary_id, model_signature],
        )?;
        Ok(())
    })
}

/// Clear all chunk and summary tombstones for a model signature. Returns the
/// total number of rows removed across both tombstone tables. Idempotent —
/// calling again with nothing left to clear returns `Ok(0)`.
///
/// # Errors
/// Returns `Err` if `model_signature` fails `validate_reembed_skip_key` or
/// either `DELETE` fails.
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
pub const REEMBED_SKIP_KEY_MAX_LEN: usize = 2048;

/// Validate and trim a `chunk_id` / `model_signature` value before it reaches
/// a reembed-skipped table query. `label` is only used to make the error
/// message identify which argument failed.
///
/// # Errors
/// Returns `Err` if `value`, after trimming, is empty, exceeds
/// `REEMBED_SKIP_KEY_MAX_LEN` bytes, or contains a NUL byte.
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

/// Little-endian `f32` vector → `BLOB`. The inverse of `embedding_from_blob`.
pub fn embedding_to_blob(embedding: &[f32]) -> Vec<u8> {
    embedding.iter().flat_map(|f| f.to_le_bytes()).collect()
}
