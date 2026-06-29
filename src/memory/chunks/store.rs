//! SQLite-backed persistence for ingested chunks.
//!
//! The store lives at `<workspace>/memory_tree/chunks.db`. Schema is applied
//! lazily on first access via [`with_connection`], so the DB is created on
//! demand without an explicit migration step.
//!
//! Upsert semantics: writes are idempotent on `chunk.id` so re-ingesting the
//! same raw source yields no duplicates.

use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{params, OptionalExtension, Transaction};
use std::collections::HashMap;

use super::connection::with_connection;
use super::types::{Chunk, Metadata, SourceKind, SourceRef, StagedChunk};
use crate::memory::config::MemoryConfig;

pub(super) const DEFAULT_LIST_LIMIT: usize = 100;
pub(super) const MAX_LIST_LIMIT: usize = 10_000;

/// Chunk lifecycle: freshly persisted, awaiting the async extract job.
pub const CHUNK_STATUS_PENDING_EXTRACTION: &str = "pending_extraction";
/// Chunk lifecycle: extract ran and the chunk passed admission.
pub const CHUNK_STATUS_ADMITTED: &str = "admitted";
/// Chunk lifecycle: appended to the L0 buffer of its source tree.
pub const CHUNK_STATUS_BUFFERED: &str = "buffered";
/// Chunk lifecycle: rolled into a sealed L1 summary.
pub const CHUNK_STATUS_SEALED: &str = "sealed";
/// Chunk lifecycle: rejected by the admission gate (too low signal).
pub const CHUNK_STATUS_DROPPED: &str = "dropped";

/// Upsert a batch of chunks atomically.
///
/// Returns the number of rows inserted or replaced. Duplicates on `chunk.id`
/// are replaced, making the operation idempotent for re-ingest of the same raw
/// source. Existing embeddings (held in the sidecar table) are preserved.
pub fn upsert_chunks(config: &MemoryConfig, chunks: &[Chunk]) -> Result<usize> {
    if chunks.is_empty() {
        return Ok(0);
    }
    with_connection(config, |conn| {
        let tx = conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare(UPSERT_SQL)?;
            upsert_chunks_with_statement(&mut stmt, chunks)?;
        }
        tx.commit()?;
        Ok(chunks.len())
    })
}

const UPSERT_SQL: &str = "INSERT INTO mem_tree_chunks (
        id, source_kind, source_id, path_scope, source_ref, owner,
        timestamp_ms, time_range_start_ms, time_range_end_ms,
        tags_json, content, token_count, seq_in_source, created_at_ms
    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
    ON CONFLICT(id) DO UPDATE SET
        source_kind = excluded.source_kind,
        source_id = excluded.source_id,
        path_scope = excluded.path_scope,
        source_ref = excluded.source_ref,
        owner = excluded.owner,
        timestamp_ms = excluded.timestamp_ms,
        time_range_start_ms = excluded.time_range_start_ms,
        time_range_end_ms = excluded.time_range_end_ms,
        tags_json = excluded.tags_json,
        content = excluded.content,
        token_count = excluded.token_count,
        seq_in_source = excluded.seq_in_source,
        created_at_ms = excluded.created_at_ms";

fn upsert_chunks_with_statement(
    stmt: &mut rusqlite::Statement<'_>,
    chunks: &[Chunk],
) -> Result<()> {
    for chunk in chunks {
        stmt.execute(params![
            chunk.id,
            chunk.metadata.source_kind.as_str(),
            chunk.metadata.source_id,
            chunk.metadata.path_scope,
            chunk.metadata.source_ref.as_ref().map(|r| r.value.as_str()),
            chunk.metadata.owner,
            chunk.metadata.timestamp.timestamp_millis(),
            chunk.metadata.time_range.0.timestamp_millis(),
            chunk.metadata.time_range.1.timestamp_millis(),
            serde_json::to_string(&chunk.metadata.tags)?,
            chunk.content,
            chunk.token_count,
            chunk.seq_in_source,
            chunk.created_at.timestamp_millis(),
        ])?;
    }
    Ok(())
}

/// Upsert staged chunks (with `content_path` + `content_sha256`) using an
/// existing transaction. The `content` column receives a ≤500-char plain-text
/// preview of the body; the full body lives on disk at `content_path`.
#[allow(dead_code)]
pub(crate) fn upsert_staged_chunks_tx(
    tx: &Transaction<'_>,
    staged: &[StagedChunk],
) -> Result<usize> {
    if staged.is_empty() {
        return Ok(0);
    }
    let mut stmt = tx.prepare(
        "INSERT INTO mem_tree_chunks (
            id, source_kind, source_id, path_scope, source_ref, owner,
            timestamp_ms, time_range_start_ms, time_range_end_ms,
            tags_json, content, token_count, seq_in_source, created_at_ms,
            content_path, content_sha256
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
        ON CONFLICT(id) DO UPDATE SET
            source_kind = excluded.source_kind,
            source_id = excluded.source_id,
            path_scope = excluded.path_scope,
            source_ref = excluded.source_ref,
            owner = excluded.owner,
            timestamp_ms = excluded.timestamp_ms,
            time_range_start_ms = excluded.time_range_start_ms,
            time_range_end_ms = excluded.time_range_end_ms,
            tags_json = excluded.tags_json,
            content = excluded.content,
            token_count = excluded.token_count,
            seq_in_source = excluded.seq_in_source,
            created_at_ms = excluded.created_at_ms,
            content_path = excluded.content_path,
            content_sha256 = excluded.content_sha256",
    )?;
    for s in staged {
        let chunk = &s.chunk;
        let preview: String = chunk.content.chars().take(500).collect();
        stmt.execute(params![
            chunk.id,
            chunk.metadata.source_kind.as_str(),
            chunk.metadata.source_id,
            chunk.metadata.path_scope,
            chunk.metadata.source_ref.as_ref().map(|r| r.value.as_str()),
            chunk.metadata.owner,
            chunk.metadata.timestamp.timestamp_millis(),
            chunk.metadata.time_range.0.timestamp_millis(),
            chunk.metadata.time_range.1.timestamp_millis(),
            serde_json::to_string(&chunk.metadata.tags)?,
            preview,
            chunk.token_count,
            chunk.seq_in_source,
            chunk.created_at.timestamp_millis(),
            s.content_path,
            s.content_sha256,
        ])?;
    }
    Ok(staged.len())
}

const SELECT_COLUMNS: &str = "id, source_kind, source_id, path_scope, source_ref, owner,
    timestamp_ms, time_range_start_ms, time_range_end_ms,
    tags_json, content, token_count, seq_in_source, created_at_ms";

/// Fetch one chunk by its id.
pub fn get_chunk(config: &MemoryConfig, id: &str) -> Result<Option<Chunk>> {
    with_connection(config, |conn| {
        let mut stmt = conn.prepare(&format!(
            "SELECT {SELECT_COLUMNS} FROM mem_tree_chunks WHERE id = ?1"
        ))?;
        let row = stmt
            .query_row(params![id], row_to_chunk)
            .optional()
            .context("Failed to query chunk by id")?;
        Ok(row)
    })
}

/// Defensive cap for batched `IN (?,?,…)` reads, well below SQLite's
/// `SQLITE_MAX_VARIABLE_NUMBER` (32 766).
const MAX_FETCH_BATCH: usize = 500;

/// Batched read of full chunk rows by id. The returned map contains only ids
/// that exist in `mem_tree_chunks`; missing ids are silently absent.
pub fn get_chunks_batch(
    config: &MemoryConfig,
    chunk_ids: &[String],
) -> Result<HashMap<String, Chunk>> {
    if chunk_ids.is_empty() {
        return Ok(HashMap::new());
    }
    with_connection(config, |conn| {
        let mut out: HashMap<String, Chunk> = HashMap::with_capacity(chunk_ids.len());
        for window in chunk_ids.chunks(MAX_FETCH_BATCH) {
            let placeholders = (1..=window.len())
                .map(|i| format!("?{i}"))
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT {SELECT_COLUMNS} FROM mem_tree_chunks WHERE id IN ({placeholders})"
            );
            let mut stmt = conn.prepare(&sql).context("prepare get_chunks_batch")?;
            let params: Vec<&dyn rusqlite::ToSql> =
                window.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
            let rows = stmt
                .query_map(params.as_slice(), row_to_chunk)
                .context("query get_chunks_batch")?;
            for row in rows {
                let chunk = row.context("decode get_chunks_batch row")?;
                out.insert(chunk.id.clone(), chunk);
            }
        }
        Ok(out)
    })
}

/// Query parameters for [`list_chunks`]. All fields are optional filters —
/// callers pass `ListChunksQuery::default()` to get recent-across-everything.
#[derive(Debug, Default, Clone)]
pub struct ListChunksQuery {
    /// Restrict to one source kind.
    pub source_kind: Option<SourceKind>,
    /// Restrict to one logical source id.
    pub source_id: Option<String>,
    /// Restrict to one owner.
    pub owner: Option<String>,
    /// Inclusive lower bound on `timestamp` (milliseconds since epoch).
    pub since_ms: Option<i64>,
    /// Inclusive upper bound on `timestamp` (milliseconds since epoch).
    pub until_ms: Option<i64>,
    /// Max rows to return (default 100 when `None`).
    pub limit: Option<usize>,
    /// Per-profile memory-source allowlist. When `Some`, memory-source chunks
    /// whose source identifier is not in the set are dropped *before* the row
    /// limit is applied. Non-source chunks always pass.
    pub source_scope: Option<std::collections::HashSet<String>>,
    /// When `true`, rows the admission gate rejected (`lifecycle_status =
    /// 'dropped'`) are excluded.
    pub exclude_dropped: bool,
}

/// List chunks matching the provided filters, ordered by `timestamp` DESC.
pub fn list_chunks(config: &MemoryConfig, query: &ListChunksQuery) -> Result<Vec<Chunk>> {
    with_connection(config, |conn| {
        let mut sql = format!("SELECT {SELECT_COLUMNS} FROM mem_tree_chunks WHERE 1=1");
        let mut bound: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if let Some(kind) = query.source_kind {
            sql.push_str(" AND source_kind = ?");
            bound.push(Box::new(kind.as_str().to_string()));
        }
        if let Some(ref source_id) = query.source_id {
            sql.push_str(" AND source_id = ?");
            bound.push(Box::new(source_id.clone()));
        }
        if let Some(ref owner) = query.owner {
            sql.push_str(" AND owner = ?");
            bound.push(Box::new(owner.clone()));
        }
        if let Some(since_ms) = query.since_ms {
            sql.push_str(" AND timestamp_ms >= ?");
            bound.push(Box::new(since_ms));
        }
        if let Some(until_ms) = query.until_ms {
            sql.push_str(" AND timestamp_ms <= ?");
            bound.push(Box::new(until_ms));
        }
        if query.exclude_dropped {
            sql.push_str(" AND lifecycle_status != ?");
            bound.push(Box::new(CHUNK_STATUS_DROPPED.to_string()));
        }
        let requested_limit = normalized_limit(query.limit);
        // When a profile source-scope is active, fetch a wider candidate set and
        // apply the gate in Rust *before* truncating, so a disallowed-source
        // prefix can't push permitted rows past the requested limit.
        let sql_limit = if query.source_scope.is_some() {
            MAX_LIST_LIMIT as i64
        } else {
            requested_limit
        };
        sql.push_str(" ORDER BY timestamp_ms DESC, seq_in_source ASC LIMIT ?");
        bound.push(Box::new(sql_limit));

        let mut stmt = conn.prepare(&sql)?;
        let param_refs: Vec<&dyn rusqlite::ToSql> = bound
            .iter()
            .map(|b| b.as_ref() as &dyn rusqlite::ToSql)
            .collect();
        let mut rows = stmt
            .query_map(param_refs.as_slice(), row_to_chunk)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("Failed to collect chunks")?;
        if let Some(ref allowed) = query.source_scope {
            rows.retain(|c| {
                super::chunk_source_allowed_in(allowed, &c.metadata.tags, &c.metadata.source_id)
            });
            rows.truncate(requested_limit as usize);
        }
        Ok(rows)
    })
}

/// Count total chunks in the store.
pub fn count_chunks(config: &MemoryConfig) -> Result<u64> {
    with_connection(config, |conn| {
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM mem_tree_chunks", [], |r| r.get(0))?;
        Ok(n.max(0) as u64)
    })
}

/// Extraction coverage — the fraction of chunks that have at least one indexed
/// entity in `mem_tree_entity_index`, in `[0.0, 1.0]`. Returns `0.0` when there
/// are no chunks.
pub fn extraction_coverage(config: &MemoryConfig) -> Result<f32> {
    with_connection(config, |conn| {
        let total: i64 =
            conn.query_row("SELECT COUNT(*) FROM mem_tree_chunks", [], |r| r.get(0))?;
        if total <= 0 {
            return Ok(0.0);
        }
        let covered: i64 = conn.query_row(
            "SELECT COUNT(*) FROM mem_tree_chunks c
              WHERE EXISTS (
                  SELECT 1 FROM mem_tree_entity_index e WHERE e.node_id = c.id
              )",
            [],
            |r| r.get(0),
        )?;
        Ok((covered.max(0) as f32) / (total as f32))
    })
}

pub(super) fn normalized_limit(requested: Option<usize>) -> i64 {
    let clamped = requested
        .unwrap_or(DEFAULT_LIST_LIMIT)
        .clamp(1, MAX_LIST_LIMIT);
    i64::try_from(clamped).unwrap_or(MAX_LIST_LIMIT as i64)
}

pub(super) fn row_to_chunk(row: &rusqlite::Row<'_>) -> rusqlite::Result<Chunk> {
    let id: String = row.get(0)?;
    let source_kind_s: String = row.get(1)?;
    let source_id: String = row.get(2)?;
    let path_scope: Option<String> = row.get(3)?;
    let source_ref: Option<String> = row.get(4)?;
    let owner: String = row.get(5)?;
    let ts_ms: i64 = row.get(6)?;
    let trs_ms: i64 = row.get(7)?;
    let tre_ms: i64 = row.get(8)?;
    let tags_json: String = row.get(9)?;
    let content: String = row.get(10)?;
    let token_count: i64 = row.get(11)?;
    let seq: i64 = row.get(12)?;
    let created_ms: i64 = row.get(13)?;

    let source_kind = SourceKind::parse(&source_kind_s).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, e.into())
    })?;
    let timestamp = ms_to_utc(ts_ms)?;
    let time_range = (ms_to_utc(trs_ms)?, ms_to_utc(tre_ms)?);
    let created_at = ms_to_utc(created_ms)?;
    let tags: Vec<String> = serde_json::from_str(&tags_json).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(8, rusqlite::types::Type::Text, Box::new(e))
    })?;

    Ok(Chunk {
        id,
        content,
        metadata: Metadata {
            source_kind,
            source_id,
            owner,
            timestamp,
            time_range,
            tags,
            source_ref: source_ref.map(SourceRef::new),
            path_scope,
        },
        token_count: token_count.max(0) as u32,
        seq_in_source: seq.max(0) as u32,
        created_at,
        // partial_message is a transient chunker signal, not stored in SQLite.
        partial_message: false,
    })
}

pub(super) fn ms_to_utc(ms: i64) -> rusqlite::Result<DateTime<Utc>> {
    Utc.timestamp_millis_opt(ms).single().ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Integer,
            format!("invalid timestamp ms {ms}").into(),
        )
    })
}

// ── Source ingest gate + lifecycle status + raw-file gate ────────────────────

pub use super::store_sources::{
    claim_source_ingest_tx, count_chunks_by_lifecycle_status, count_raw_paths_ingested_with_prefix,
    filter_raw_paths_not_ingested, get_chunk_lifecycle_status, is_source_ingested,
    mark_raw_paths_ingested, set_chunk_lifecycle_status, RAW_FILE_GATE_KIND,
};
