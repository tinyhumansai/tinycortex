//! Persistence for sealed summary rows (`mem_tree_summaries`) and their
//! per-model embedding sidecar (`mem_tree_summary_embeddings`).
//!
//! Summary rows are immutable once emitted. The legacy `embedding` blob column
//! on `mem_tree_summaries` is always written NULL; vectors live in the
//! per-signature sidecar table so multiple embedding models can coexist.

use std::collections::HashMap;

use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{params, OptionalExtension, Transaction};

use super::common::{decode_signature_blob, ms_to_utc, pack_embedding_blob};
use super::types::{SummaryNode, TreeKind};
use crate::memory::chunks::{tree_active_signature, with_connection};
use crate::memory::config::MemoryConfig;
use crate::memory::score::embed::decode_optional_blob;

/// Insert a sealed summary. Immutable; the caller mints a fresh id per seal.
/// Idempotent on the primary key (`INSERT OR IGNORE`).
///
/// The legacy `embedding` column is written NULL — when `node.embedding` is
/// `Some`, the vector is persisted to the per-model sidecar at
/// `model_signature`, in this same transaction so it commits atomically.
pub fn insert_summary_tx(
    tx: &Transaction<'_>,
    node: &SummaryNode,
    model_signature: &str,
) -> Result<()> {
    let embedding_blob: Option<Vec<u8>> = None;
    tx.execute(
        "INSERT OR IGNORE INTO mem_tree_summaries (
            id, tree_id, tree_kind, level, parent_id,
            child_ids_json, content, token_count,
            entities_json, topics_json,
            time_range_start_ms, time_range_end_ms,
            score, sealed_at_ms, deleted, embedding,
            doc_id, version_ms
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
        params![
            node.id,
            node.tree_id,
            node.tree_kind.as_str(),
            node.level,
            node.parent_id,
            serde_json::to_string(&node.child_ids)?,
            node.content,
            node.token_count,
            serde_json::to_string(&node.entities)?,
            serde_json::to_string(&node.topics)?,
            node.time_range_start.timestamp_millis(),
            node.time_range_end.timestamp_millis(),
            node.score,
            node.sealed_at.timestamp_millis(),
            node.deleted as i64,
            embedding_blob,
            node.doc_id,
            node.version_ms,
        ],
    )
    .with_context(|| format!("Failed to insert summary id={}", node.id))?;

    if let Some(v) = node.embedding.as_deref() {
        upsert_summary_embedding_conn(tx, &node.id, model_signature, v)?;
    }
    Ok(())
}

/// Fetch one summary by id. Soft-deleted rows are returned with `deleted = true`.
pub fn get_summary(config: &MemoryConfig, id: &str) -> Result<Option<SummaryNode>> {
    with_connection(config, |conn| {
        let mut stmt = conn.prepare(SELECT_SUMMARY_COLS)?;
        let row = stmt
            .query_row(params![id], row_to_summary)
            .optional()
            .context("Failed to query summary by id")?;
        Ok(row)
    })
}

const MAX_FETCH_BATCH: usize = 500;

/// Fetch many summaries by id in a single round-trip per window. Missing ids
/// are silently absent.
pub fn get_summaries_batch(
    config: &MemoryConfig,
    summary_ids: &[String],
) -> Result<HashMap<String, SummaryNode>> {
    if summary_ids.is_empty() {
        return Ok(HashMap::new());
    }
    with_connection(config, |conn| {
        let mut out: HashMap<String, SummaryNode> = HashMap::with_capacity(summary_ids.len());
        for window in summary_ids.chunks(MAX_FETCH_BATCH) {
            let placeholders = (1..=window.len())
                .map(|i| format!("?{i}"))
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!("{SELECT_SUMMARY_BASE} WHERE id IN ({placeholders})");
            let mut stmt = conn.prepare(&sql)?;
            let params: Vec<&dyn rusqlite::ToSql> =
                window.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
            let rows = stmt
                .query_map(params.as_slice(), row_to_summary)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .context("Failed to collect summaries batch")?;
            for s in rows {
                out.insert(s.id.clone(), s);
            }
        }
        Ok(out)
    })
}

/// List sealed summaries for a tree at a level, ordered by `sealed_at` ASC.
/// Skips tombstoned rows.
pub fn list_summaries_at_level(
    config: &MemoryConfig,
    tree_id: &str,
    level: u32,
) -> Result<Vec<SummaryNode>> {
    with_connection(config, |conn| {
        let mut stmt = conn.prepare(&format!(
            "{SELECT_SUMMARY_BASE} WHERE tree_id = ?1 AND level = ?2 AND deleted = 0 \
             ORDER BY sealed_at_ms ASC"
        ))?;
        let rows = stmt
            .query_map(params![tree_id, level], row_to_summary)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("Failed to collect summaries")?;
        Ok(rows)
    })
}

/// List every non-deleted summary whose time-range envelope is fully contained
/// in `[since_ms, until_ms]` (inclusive), across all levels.
pub fn list_summaries_in_window(
    config: &MemoryConfig,
    tree_id: &str,
    since_ms: i64,
    until_ms: i64,
) -> Result<Vec<SummaryNode>> {
    with_connection(config, |conn| {
        let mut stmt = conn.prepare(&format!(
            "{SELECT_SUMMARY_BASE} WHERE tree_id = ?1 AND deleted = 0 \
               AND time_range_start_ms >= ?2 AND time_range_end_ms <= ?3 \
             ORDER BY level ASC, time_range_start_ms ASC"
        ))?;
        let rows = stmt
            .query_map(params![tree_id, since_ms, until_ms], row_to_summary)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("Failed to collect in-window summaries")?;
        Ok(rows)
    })
}

/// List non-deleted summaries whose `parent_id` is `parent` (its direct
/// children). Ordered by `sealed_at` ASC. Used by the tree-walk read path.
pub fn list_children_of_summary(
    config: &MemoryConfig,
    parent_id: &str,
) -> Result<Vec<SummaryNode>> {
    with_connection(config, |conn| {
        let mut stmt = conn.prepare(&format!(
            "{SELECT_SUMMARY_BASE} WHERE parent_id = ?1 AND deleted = 0 \
             ORDER BY sealed_at_ms ASC"
        ))?;
        let rows = stmt
            .query_map(params![parent_id], row_to_summary)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("Failed to collect summary children")?;
        Ok(rows)
    })
}

/// Count non-deleted summaries in a tree (diagnostic helper).
pub fn count_summaries(config: &MemoryConfig, tree_id: &str) -> Result<u64> {
    with_connection(config, |conn| {
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM mem_tree_summaries WHERE tree_id = ?1 AND deleted = 0",
                params![tree_id],
                |r| r.get(0),
            )
            .context("count summaries query")?;
        Ok(n.max(0) as u64)
    })
}

// ── Embedding sidecar ───────────────────────────────────────────────────────

fn upsert_summary_embedding_conn(
    conn: &rusqlite::Connection,
    summary_id: &str,
    model_signature: &str,
    embedding: &[f32],
) -> Result<()> {
    let blob = pack_embedding_blob(embedding);
    let dim = i64::try_from(embedding.len()).context("embedding dimension does not fit i64")?;
    let created_at = Utc::now().timestamp_millis() as f64 / 1000.0;
    conn.execute(
        "INSERT INTO mem_tree_summary_embeddings
             (summary_id, model_signature, vector, dim, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(summary_id, model_signature) DO UPDATE SET
                vector = excluded.vector, dim = excluded.dim, created_at = excluded.created_at",
        params![summary_id, model_signature, blob, dim, created_at],
    )?;
    Ok(())
}

/// Store a summary embedding for a specific provider/model/dimension signature.
pub fn set_summary_embedding_for_signature(
    config: &MemoryConfig,
    summary_id: &str,
    model_signature: &str,
    embedding: &[f32],
) -> Result<()> {
    with_connection(config, |conn| {
        upsert_summary_embedding_conn(conn, summary_id, model_signature, embedding)
    })
}

/// Store a summary embedding under the active model signature.
pub fn set_summary_embedding(
    config: &MemoryConfig,
    summary_id: &str,
    embedding: &[f32],
) -> Result<usize> {
    let signature = tree_active_signature(config);
    set_summary_embedding_for_signature(config, summary_id, &signature, embedding)?;
    Ok(1)
}

/// Fetch a summary embedding for exactly one signature.
pub fn get_summary_embedding_for_signature(
    config: &MemoryConfig,
    summary_id: &str,
    model_signature: &str,
) -> Result<Option<Vec<f32>>> {
    with_connection(config, |conn| {
        let row: Option<(Option<Vec<u8>>, i64)> = conn
            .query_row(
                "SELECT vector, dim FROM mem_tree_summary_embeddings
                  WHERE summary_id = ?1 AND model_signature = ?2",
                params![summary_id, model_signature],
                |r| Ok((Some(r.get(0)?), r.get(1)?)),
            )
            .optional()?;
        match row {
            None => Ok(None),
            Some((blob, dim)) => {
                decode_signature_blob(blob, dim, &format!("summary_id={summary_id}"))
            }
        }
    })
}

/// Fetch a summary embedding under the active model signature.
pub fn get_summary_embedding(config: &MemoryConfig, summary_id: &str) -> Result<Option<Vec<f32>>> {
    let signature = tree_active_signature(config);
    get_summary_embedding_for_signature(config, summary_id, &signature)
}

const MAX_EMBEDDING_BATCH: usize = 500;

/// Batched read of summary embeddings under one signature. The map contains
/// only ids with a non-null vector under `model_signature`.
pub fn get_summary_embeddings_for_signature_batch(
    config: &MemoryConfig,
    summary_ids: &[String],
    model_signature: &str,
) -> Result<HashMap<String, Vec<f32>>> {
    if summary_ids.is_empty() {
        return Ok(HashMap::new());
    }
    with_connection(config, |conn| {
        let mut out: HashMap<String, Vec<f32>> = HashMap::with_capacity(summary_ids.len());
        for window in summary_ids.chunks(MAX_EMBEDDING_BATCH) {
            let placeholders = std::iter::repeat_n("?", window.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT summary_id, vector, dim FROM mem_tree_summary_embeddings
                  WHERE summary_id IN ({placeholders}) AND model_signature = ?{sig_idx}",
                sig_idx = window.len() + 1,
            );
            let mut stmt = conn.prepare(&sql)?;
            let mut bound: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(window.len() + 1);
            for id in window {
                bound.push(id as &dyn rusqlite::ToSql);
            }
            bound.push(&model_signature as &dyn rusqlite::ToSql);
            let rows = stmt.query_map(bound.as_slice(), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<Vec<u8>>>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })?;
            for row in rows {
                let (summary_id, blob, dim) = row?;
                if let Some(v) =
                    decode_signature_blob(blob, dim, &format!("summary_id={summary_id}"))?
                {
                    out.insert(summary_id, v);
                }
            }
        }
        Ok(out)
    })
}

/// Batched read under the active signature.
pub fn get_summary_embeddings_batch(
    config: &MemoryConfig,
    summary_ids: &[String],
) -> Result<HashMap<String, Vec<f32>>> {
    let signature = tree_active_signature(config);
    get_summary_embeddings_for_signature_batch(config, summary_ids, &signature)
}

// ── Row decoding ────────────────────────────────────────────────────────────

const SELECT_SUMMARY_BASE: &str = "SELECT id, tree_id, tree_kind, level, parent_id, \
    child_ids_json, content, token_count, entities_json, topics_json, \
    time_range_start_ms, time_range_end_ms, score, sealed_at_ms, deleted, embedding, \
    doc_id, version_ms FROM mem_tree_summaries";

const SELECT_SUMMARY_COLS: &str =
    "SELECT id, tree_id, tree_kind, level, parent_id, child_ids_json, content, token_count, \
     entities_json, topics_json, time_range_start_ms, time_range_end_ms, score, sealed_at_ms, \
     deleted, embedding, doc_id, version_ms FROM mem_tree_summaries WHERE id = ?1";

fn row_to_summary(row: &rusqlite::Row<'_>) -> rusqlite::Result<SummaryNode> {
    let id: String = row.get(0)?;
    let tree_id: String = row.get(1)?;
    let tree_kind_s: String = row.get(2)?;
    let level: i64 = row.get(3)?;
    let parent_id: Option<String> = row.get(4)?;
    let child_ids_json: String = row.get(5)?;
    let content: String = row.get(6)?;
    let token_count: i64 = row.get(7)?;
    let entities_json: String = row.get(8)?;
    let topics_json: String = row.get(9)?;
    let trs_ms: i64 = row.get(10)?;
    let tre_ms: i64 = row.get(11)?;
    let score: f64 = row.get(12)?;
    let sealed_ms: i64 = row.get(13)?;
    let deleted: i64 = row.get(14)?;
    let embedding_blob: Option<Vec<u8>> = row.get(15)?;
    let doc_id: Option<String> = row.get(16)?;
    let version_ms: Option<i64> = row.get(17)?;

    let tree_kind = TreeKind::parse(&tree_kind_s).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, e.into())
    })?;
    let child_ids: Vec<String> = serde_json::from_str(&child_ids_json).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(5, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let entities: Vec<String> = serde_json::from_str(&entities_json).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(8, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let topics: Vec<String> = serde_json::from_str(&topics_json).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(9, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let embedding =
        decode_optional_blob(embedding_blob, &format!("summary_id={id}")).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                15,
                rusqlite::types::Type::Blob,
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    e.to_string(),
                )),
            )
        })?;

    Ok(SummaryNode {
        id,
        tree_id,
        tree_kind,
        level: level.max(0) as u32,
        parent_id,
        child_ids,
        content,
        token_count: token_count.max(0) as u32,
        entities,
        topics,
        time_range_start: ms_to_utc(trs_ms)?,
        time_range_end: ms_to_utc(tre_ms)?,
        score: score as f32,
        sealed_at: ms_to_utc(sealed_ms)?,
        deleted: deleted != 0,
        embedding,
        doc_id,
        version_ms,
    })
}
