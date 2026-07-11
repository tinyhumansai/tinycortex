//! Source-level ingest gates, chunk lifecycle status, and the raw-archive file
//! coverage gate — all stored in `mem_tree_ingested_sources` + the
//! `lifecycle_status` column.

use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension, Transaction};

use super::connection::with_connection;
use super::types::SourceKind;
use crate::memory::config::MemoryConfig;

// ── Lifecycle status ─────────────────────────────────────────────────────────

/// Set the lifecycle status column for `chunk_id`. See the
/// `super::store::CHUNK_STATUS_*` constants for valid values — this function
/// does not validate `status` against them, so passing an arbitrary string
/// stores it as-is.
///
/// # Errors
/// Returns `Err` only if the underlying `UPDATE` fails. Silently affects zero
/// rows (no error, no signal) if `chunk_id` does not exist — see the NOTE on
/// [`set_chunk_lifecycle_status_conn`].
pub fn set_chunk_lifecycle_status(
    config: &MemoryConfig,
    chunk_id: &str,
    status: &str,
) -> Result<()> {
    with_connection(config, |conn| {
        set_chunk_lifecycle_status_conn(conn, chunk_id, status)
    })
}

/// Set the lifecycle status column inside a caller-owned transaction. The
/// caller commits/rolls back `tx`.
///
/// # Errors
/// See [`set_chunk_lifecycle_status`].
#[allow(dead_code)]
pub(crate) fn set_chunk_lifecycle_status_tx(
    tx: &Transaction<'_>,
    chunk_id: &str,
    status: &str,
) -> Result<()> {
    set_chunk_lifecycle_status_conn(tx, chunk_id, status)
}

/// Core `UPDATE ... SET lifecycle_status` over an arbitrary `&Connection`.
///
/// # NOTE
/// `changed` (the affected-row count) is computed but never inspected — a
/// call for a nonexistent `chunk_id` silently succeeds with zero rows
/// touched rather than surfacing that as an error or a return value the
/// caller could check. Callers cannot currently distinguish "status set" from
/// "chunk_id did not exist" without a separate existence check.
///
/// # Errors
/// Returns `Err` only if the `UPDATE` statement itself fails.
fn set_chunk_lifecycle_status_conn(conn: &Connection, chunk_id: &str, status: &str) -> Result<()> {
    let changed = conn.execute(
        "UPDATE mem_tree_chunks SET lifecycle_status = ?1 WHERE id = ?2",
        params![status, chunk_id],
    )?;
    if changed == 0 {}
    Ok(())
}

/// Read the lifecycle status column for `chunk_id`, or `None` if the row is absent.
///
/// # Errors
/// Returns `Err` only if the underlying query fails.
pub fn get_chunk_lifecycle_status(config: &MemoryConfig, chunk_id: &str) -> Result<Option<String>> {
    with_connection(config, |conn| {
        let row = conn
            .query_row(
                "SELECT lifecycle_status FROM mem_tree_chunks WHERE id = ?1",
                params![chunk_id],
                |r| r.get::<_, String>(0),
            )
            .optional()?;
        Ok(row)
    })
}

/// Count chunks currently sitting at a given lifecycle status. Matches
/// exactly (no prefix/wildcard semantics) — an unknown status string simply
/// counts as `0`.
///
/// # Errors
/// Returns `Err` only if the underlying query fails.
pub fn count_chunks_by_lifecycle_status(config: &MemoryConfig, status: &str) -> Result<u64> {
    with_connection(config, |conn| {
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM mem_tree_chunks WHERE lifecycle_status = ?1",
            params![status],
            |r| r.get(0),
        )?;
        Ok(n.max(0) as u64)
    })
}

// ── Source ingest gate ───────────────────────────────────────────────────────

/// Best-effort, non-transactional check used to skip canonicalisation when a
/// source has already been ingested. The authoritative gate is
/// [`claim_source_ingest_tx`].
///
/// Because this runs outside any transaction, a `true`/`false` answer here
/// can be stale by the time the caller acts on it under concurrent ingest —
/// callers that need a correctness guarantee (not just a fast-path skip)
/// must still go through [`claim_source_ingest_tx`] inside their own persist
/// transaction.
///
/// # Errors
/// Returns `Err` only if the underlying query fails.
pub fn is_source_ingested(
    config: &MemoryConfig,
    source_kind: SourceKind,
    source_id: &str,
) -> Result<bool> {
    with_connection(config, |conn| {
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM mem_tree_ingested_sources \
             WHERE source_kind = ?1 AND source_id = ?2",
            params![source_kind.as_str(), source_id],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    })
}

/// Atomically claim `(source_kind, source_id)` for ingestion. Returns `true` if
/// the row was newly inserted; `false` if a previous ingest already claimed it.
/// Lives inside the persist transaction so two concurrent ingests of the same
/// source can't both pass the gate.
///
/// Idempotent by design: repeated calls for an already-claimed
/// `(source_kind, source_id)` keep returning `false` and never overwrite
/// `ingested_at_ms` (the `INSERT OR IGNORE` is a no-op on conflict).
///
/// # Errors
/// Returns `Err` only if the `INSERT OR IGNORE` statement fails. Does not
/// commit `tx` — the caller owns the surrounding transaction.
pub fn claim_source_ingest_tx(
    tx: &Transaction<'_>,
    source_kind: SourceKind,
    source_id: &str,
    now_ms: i64,
) -> Result<bool> {
    let inserted = tx.execute(
        "INSERT OR IGNORE INTO mem_tree_ingested_sources \
            (source_kind, source_id, ingested_at_ms) \
         VALUES (?1, ?2, ?3)",
        params![source_kind.as_str(), source_id, now_ms],
    )?;
    Ok(inserted > 0)
}

// ── Raw-archive file coverage gate ───────────────────────────────────────────

/// `source_kind` value used in `mem_tree_ingested_sources` to record that a raw
/// archive file (relative path under `<content_root>/`) has been covered by a
/// tree summary. Distinct from the chunk-store [`SourceKind`] values.
pub const RAW_FILE_GATE_KIND: &str = "raw_file";

/// Record that the given raw archive files are covered by a tree summary.
/// Idempotent (`INSERT OR IGNORE`); returns the number of newly-recorded
/// paths (already-recorded paths in `rel_paths` are silently skipped and not
/// counted).
///
/// # Errors
/// Returns `Err` if beginning/committing the transaction or preparing/
/// executing the insert for any path fails. Returns `Ok(0)` immediately (no
/// DB access) for an empty `rel_paths`.
pub fn mark_raw_paths_ingested(config: &MemoryConfig, rel_paths: &[String]) -> Result<u64> {
    if rel_paths.is_empty() {
        return Ok(0);
    }
    let now_ms = Utc::now().timestamp_millis();
    with_connection(config, |conn| {
        let tx = conn.unchecked_transaction()?;
        let mut inserted: u64 = 0;
        {
            let mut stmt = tx.prepare(
                "INSERT OR IGNORE INTO mem_tree_ingested_sources \
                    (source_kind, source_id, ingested_at_ms) \
                 VALUES (?1, ?2, ?3)",
            )?;
            for path in rel_paths {
                inserted += stmt.execute(params![RAW_FILE_GATE_KIND, path, now_ms])? as u64;
            }
        }
        tx.commit()?;
        Ok(inserted)
    })
}

/// Filter `rel_paths` down to the ones NOT yet recorded as ingested raw files.
/// Order of the surviving paths is preserved.
///
/// Issues one `COUNT(*)` query per path (not batched) — cost scales linearly
/// with `rel_paths.len()`, each a separate round-trip against the reused
/// prepared statement.
///
/// # Errors
/// Returns `Err` if preparing the statement or executing it for any path
/// fails. Returns `Ok(Vec::new())` immediately (no DB access) for an empty
/// `rel_paths`.
pub fn filter_raw_paths_not_ingested(
    config: &MemoryConfig,
    rel_paths: &[String],
) -> Result<Vec<String>> {
    if rel_paths.is_empty() {
        return Ok(Vec::new());
    }
    with_connection(config, |conn| {
        let mut stmt = conn.prepare(
            "SELECT COUNT(*) FROM mem_tree_ingested_sources \
             WHERE source_kind = ?1 AND source_id = ?2",
        )?;
        let mut out: Vec<String> = Vec::new();
        for path in rel_paths {
            let n: i64 = stmt.query_row(params![RAW_FILE_GATE_KIND, path], |r| r.get(0))?;
            if n == 0 {
                out.push(path.clone());
            }
        }
        Ok(out)
    })
}

/// Count raw-file gate rows whose path starts with `rel_prefix`. Rust-side
/// prefix filter (not SQL `LIKE`) so `_` / `%` in slugs are treated literally.
///
/// Scans every `raw_file`-kind gate row in `mem_tree_ingested_sources` — cost
/// scales with total gate-row count, not with the number of matching paths.
///
/// # Errors
/// Returns `Err` if preparing/executing the query or reading any row fails.
pub fn count_raw_paths_ingested_with_prefix(
    config: &MemoryConfig,
    rel_prefix: &str,
) -> Result<u64> {
    with_connection(config, |conn| {
        let mut stmt =
            conn.prepare("SELECT source_id FROM mem_tree_ingested_sources WHERE source_kind = ?1")?;
        let rows = stmt.query_map(params![RAW_FILE_GATE_KIND], |r| r.get::<_, String>(0))?;
        let mut n: u64 = 0;
        for row in rows {
            if row?.starts_with(rel_prefix) {
                n += 1;
            }
        }
        Ok(n)
    })
}
