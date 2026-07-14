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
/// Returns the number of rows inserted or replaced (always `chunks.len()`,
/// even if some of those ids already existed — this counts attempted writes,
/// not "newly created" rows). Duplicates on `chunk.id` are replaced, making
/// the operation idempotent for re-ingest of the same raw source. Existing
/// embeddings (held in the sidecar table) are preserved because they key on
/// `chunk_id` in a separate table this statement never touches.
///
/// Returns `Ok(0)` immediately (no DB access) for an empty `chunks` slice.
///
/// Replacing a previously staged row clears its file pointer and restores the
/// lifecycle to `admitted`, so the row's storage representation and lifecycle
/// match this plain-content write.
///
/// # Errors
/// Returns `Err` if beginning the transaction, preparing/executing
/// `UPSERT_SQL` for any chunk, or committing fails. On error, no partial
/// writes are visible (the whole batch rolls back with the transaction).
pub fn upsert_chunks(config: &MemoryConfig, chunks: &[Chunk]) -> Result<usize> {
    if chunks.is_empty() {
        return Ok(0);
    }
    let (count, replaced_content_paths) = with_connection(config, |conn| {
        let tx = conn.unchecked_transaction()?;
        let mut replaced_content_paths = Vec::new();
        for chunk in chunks {
            if let Some(path) = tx
                .query_row(
                    "SELECT content_path FROM mem_tree_chunks WHERE id = ?1",
                    [&chunk.id],
                    |row| row.get::<_, Option<String>>(0),
                )
                .optional()?
                .flatten()
                .filter(|path| !path.is_empty())
            {
                replaced_content_paths.push(path);
            }
        }
        let count = upsert_chunks_tx(&tx, chunks)?;
        tx.commit()?;
        Ok((count, replaced_content_paths))
    })?;
    super::store_delete::remove_chunk_content_files(config, &replaced_content_paths);
    Ok(count)
}

pub fn upsert_chunks_tx(tx: &Transaction<'_>, chunks: &[Chunk]) -> Result<usize> {
    if chunks.is_empty() {
        return Ok(0);
    }
    let mut stmt = tx.prepare(UPSERT_SQL)?;
    upsert_chunks_with_statement(&mut stmt, chunks)?;
    Ok(chunks.len())
}

/// `INSERT ... ON CONFLICT(id) DO UPDATE` for the plain (non-staged) chunk
/// columns. Deliberately omits `content_path` / `content_sha256` /
/// `lifecycle_status` — see the gotcha on [`upsert_chunks`].
const UPSERT_SQL: &str = "INSERT INTO mem_tree_chunks (
        id, source_kind, source_id, path_scope, source_ref, owner,
        timestamp_ms, time_range_start_ms, time_range_end_ms,
        tags_json, content, token_count, seq_in_source, created_at_ms, lifecycle_status
    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
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
        content_path = NULL,
        content_sha256 = NULL,
        lifecycle_status = excluded.lifecycle_status";

/// Bind and execute `UPSERT_SQL` once per chunk against an already-prepared
/// statement. Split out from [`upsert_chunks`] so the statement is prepared
/// exactly once per batch rather than once per chunk.
///
/// # Errors
/// Returns `Err` if serializing `chunk.metadata.tags` to JSON fails
/// (practically unreachable — `Vec<String>` has no fallible `Serialize`
/// path) or if any `stmt.execute` call fails.
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
            CHUNK_STATUS_ADMITTED,
        ])?;
    }
    Ok(())
}

/// Upsert staged chunks (with `content_path` + `content_sha256`) using an
/// existing transaction. The `content` column receives a ≤500-char plain-text
/// preview of the body; the full body lives on disk at `content_path`.
///
/// Unlike [`upsert_chunks`]'s `UPSERT_SQL`, this statement's `ON CONFLICT`
/// clause *does* overwrite `content_path` and `content_sha256` — it is the
/// counterpart that keeps those columns in sync when re-staging an existing
/// chunk id.
///
/// Writing the on-disk body file at `content_path` (and verifying/updating
/// `content_sha256`) is the caller's responsibility; this function only
/// updates the SQLite row and assumes the file already matches.
///
/// Currently unused outside this crate's own callers (`#[allow(dead_code)]`)
/// — reserved for the staged-content write path once it lands.
///
/// # Errors
/// Returns `Err` if preparing the statement fails, if serializing any
/// chunk's tags to JSON fails, or if any `stmt.execute` call fails. Does not
/// commit `tx` itself — the caller owns the transaction lifecycle.
#[allow(dead_code)]
pub fn upsert_staged_chunks_tx(tx: &Transaction<'_>, staged: &[StagedChunk]) -> Result<usize> {
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

/// Column list shared by every plain-chunk `SELECT` in this module. Ordinal
/// positions here are load-bearing: `row_to_chunk` reads columns by
/// numeric index, so this list and that function's `row.get(N)` calls must
/// stay in lockstep.
pub fn update_chunk_content_sha256(
    config: &MemoryConfig,
    chunk_id: &str,
    new_sha256: &str,
) -> Result<()> {
    with_connection(config, |conn| {
        conn.execute(
            "UPDATE mem_tree_chunks SET content_sha256 = ?1 WHERE id = ?2",
            params![new_sha256, chunk_id],
        )?;
        Ok(())
    })
}

pub fn update_summary_content_sha256(
    config: &MemoryConfig,
    summary_id: &str,
    new_sha256: &str,
) -> Result<()> {
    with_connection(config, |conn| {
        conn.execute(
            "UPDATE mem_tree_summaries SET content_sha256 = ?1 WHERE id = ?2",
            params![new_sha256, summary_id],
        )?;
        Ok(())
    })
}

pub fn list_source_ids_with_prefix(
    config: &MemoryConfig,
    source_kind: SourceKind,
    prefix: &str,
) -> Result<Vec<String>> {
    with_connection(config, |conn| {
        let mut stmt = conn.prepare(
            "SELECT DISTINCT source_id FROM mem_tree_chunks
             WHERE source_kind = ?1 ORDER BY source_id ASC",
        )?;
        let rows = stmt.query_map(params![source_kind.as_str()], |row| row.get::<_, String>(0))?;
        Ok(rows
            .filter_map(std::result::Result::ok)
            .filter(|id| id.starts_with(prefix))
            .collect())
    })
}

pub(super) const SELECT_COLUMNS: &str = "id, source_kind, source_id, path_scope, source_ref, owner,
    timestamp_ms, time_range_start_ms, time_range_end_ms,
    tags_json, content, token_count, seq_in_source, created_at_ms";

/// Fetch one chunk by its id.
///
/// # Errors
/// Returns `Err` if the query fails or the row fails to decode (see
/// `row_to_chunk` — malformed `source_kind`, `tags_json`, or timestamp
/// values). Returns `Ok(None)`, not an error, when `id` does not exist.
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
///
/// `chunk_ids` is split into windows of at most `MAX_FETCH_BATCH` so a
/// single query never approaches SQLite's bound-parameter limit.
///
/// # Errors
/// Returns `Err` if any window's query preparation, execution, or row
/// decoding (`row_to_chunk`) fails. Returns `Ok(HashMap::new())`
/// immediately (no DB access) when `chunk_ids` is empty.
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

/// Count total chunks in the store (no filters — every row in
/// `mem_tree_chunks`, regardless of lifecycle status).
///
/// # Errors
/// Returns `Err` only if the underlying query fails.
pub fn count_chunks(config: &MemoryConfig) -> Result<u64> {
    with_connection(config, |conn| {
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM mem_tree_chunks", [], |r| r.get(0))?;
        Ok(n.max(0) as u64)
    })
}

/// Extraction coverage — the fraction of chunks that have at least one indexed
/// entity in `mem_tree_entity_index`, in `[0.0, 1.0]`. Returns `0.0` when there
/// are no chunks.
///
/// # Gotcha (audit finding SC-6)
/// This joins against `mem_tree_entity_index` in *this* chunk database.
/// `src/memory/store/entity_index/store.rs` opens what is nominally the same
/// table at a path its own caller controls; if that path is not this chunk
/// DB's path, entity-index writes made through that module never show up
/// here, and this always computes coverage against an entity_index table
/// that stays empty from this function's point of view — coverage reads as
/// permanently `0.0` regardless of actual extraction activity.
///
/// # Errors
/// Returns `Err` only if either `COUNT(*)` query fails.
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

/// Decode one `mem_tree_chunks` row (columns in exactly [`SELECT_COLUMNS`]
/// order) into a [`Chunk`]. Shared by every plain-chunk query in this module.
///
/// `token_count` / `seq_in_source` are clamped to `0` if the stored `i64` is
/// negative (defensive against a hand-edited or otherwise corrupted DB — this
/// silently coerces rather than erroring, since a negative count/seq has no
/// valid interpretation but also isn't worth failing the whole read over).
/// `partial_message` is never persisted (it's a transient chunker signal) and
/// always decodes to `false`.
///
/// # Errors
/// Returns `Err` if `source_kind` fails [`SourceKind::parse`], `tags_json`
/// fails to deserialize as `Vec<String>`, or any of the three timestamp
/// columns fails [`ms_to_utc`] (out-of-range milliseconds).
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

/// Convert milliseconds-since-epoch to a UTC timestamp.
///
/// # Errors
/// Returns `Err(rusqlite::Error::FromSqlConversionFailure)` if `ms` does not
/// map to a single valid `DateTime<Utc>` (chrono's `timestamp_millis_opt`
/// returns `None`/`Ambiguous` — out-of-range values only; every in-range
/// millisecond value is unambiguous under UTC).
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
    delete_source_ingest, filter_raw_paths_not_ingested, get_chunk_lifecycle_status,
    is_source_ingested, mark_raw_paths_ingested, set_chunk_lifecycle_status, RAW_FILE_GATE_KIND,
};
