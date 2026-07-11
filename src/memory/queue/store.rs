//! SQLite persistence for the memory-tree job queue (claim / enqueue side).
//!
//! Producers call [`enqueue`] inside their own writes (or with a fresh tx) to
//! atomically commit the side-effect plus its follow-up job. The worker pool
//! calls [`claim_next`] to lease a job; settlement (`mark_done` /
//! `mark_failed` / `mark_deferred` / recovery / requeue) lives in
//! [`super::store_settle`].
//!
//! All persistence lives in the shared `chunks.db` (opened via
//! [`crate::memory::chunks::with_connection`]) so a producer can insert its
//! side-effect and its follow-up job in one transaction. The `mem_tree_jobs`
//! table and its partial-unique dedupe index are owned by the chunks schema —
//! this module never issues DDL.
//!
//! Concurrency:
//! - The dedupe key is enforced by a partial `UNIQUE` index that only covers
//!   `status IN ('ready', 'running')`. Producers use `INSERT OR IGNORE` so a
//!   duplicate enqueue while a job is in flight or queued is a silent no-op; a
//!   duplicate enqueue after the first completes creates a fresh row.
//! - `claim_next` is one statement (`UPDATE … WHERE id = (SELECT … LIMIT 1)
//!   RETURNING …`). SQLite serialises writes, so no two workers claim the same
//!   row.

use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use uuid::Uuid;

use crate::memory::chunks::with_connection;
use crate::memory::config::MemoryConfig;
use crate::memory::queue::types::{Job, JobKind, JobStatus, NewJob, RETIRED_JOB_KINDS};

/// Default visibility lock — a worker that crashes mid-job will have its row
/// recovered after this window. 5 min is comfortably larger than any expected
/// single-job runtime without leaving real failures stuck for hours.
pub const DEFAULT_LOCK_DURATION_MS: i64 = 5 * 60 * 1_000;

/// Backoff math for retry. Returns `now + min(base * 2^attempts, cap)`.
pub(crate) const RETRY_BASE_MS: i64 = 60 * 1_000;
pub(crate) const RETRY_CAP_MS: i64 = 60 * 60 * 1_000;
pub(crate) const DEFAULT_MAX_ATTEMPTS: u32 = 5;

/// Enqueue one job. Idempotent on `dedupe_key` while another active row (status
/// `ready`/`running`) shares it. Returns `Some(id)` if inserted, `None` if a
/// duplicate was suppressed.
pub fn enqueue(config: &MemoryConfig, job: &NewJob) -> Result<Option<String>> {
    with_connection(config, |conn| enqueue_conn(conn, job))
}

/// Enqueue inside a caller-owned transaction. Use this when the producer is
/// already mid-tx (e.g. writing chunks + jobs in one commit) so the queue
/// insert lands atomically with the side-effect. `Transaction` derefs to
/// `Connection`, so callers just pass `&tx`.
pub fn enqueue_tx(tx: &Transaction<'_>, job: &NewJob) -> Result<Option<String>> {
    enqueue_conn(tx, job)
}

pub(crate) fn enqueue_conn(conn: &Connection, job: &NewJob) -> Result<Option<String>> {
    let id = format!("job:{}", Uuid::new_v4());
    let now_ms = Utc::now().timestamp_millis();
    let available_at = job.available_at_ms.unwrap_or(now_ms);
    let max_attempts = job.max_attempts.unwrap_or(DEFAULT_MAX_ATTEMPTS) as i64;

    let inserted = conn.execute(
        "INSERT OR IGNORE INTO mem_tree_jobs (
            id, kind, payload_json, dedupe_key, status, attempts, max_attempts,
            available_at_ms, locked_until_ms, last_error,
            created_at_ms, started_at_ms, completed_at_ms
        ) VALUES (?1, ?2, ?3, ?4, 'ready', 0, ?5, ?6, NULL, NULL, ?7, NULL, NULL)",
        params![
            id,
            job.kind.as_str(),
            job.payload_json,
            job.dedupe_key,
            max_attempts,
            available_at,
            now_ms,
        ],
    )?;

    if inserted == 0 {
        // Dedupe-suppressed: a row with this key is already ready/running.
        return Ok(None);
    }
    Ok(Some(id))
}

/// Atomically claim the next ready job whose `available_at_ms` has come due.
/// Sets `status=running`, bumps `attempts`, stamps `started_at_ms` and
/// `locked_until_ms`. Returns `None` when the queue is empty / not yet due.
///
/// Retired kinds (`topic_route`, `digest_daily`) are excluded from the claim so
/// a leftover old-queue row never reaches `row_to_job` (which would fail to
/// parse it). [`purge_retired_jobs`] removes such rows.
pub fn claim_next(config: &MemoryConfig, lock_duration_ms: i64) -> Result<Option<Job>> {
    with_connection(config, |conn| {
        let now_ms = Utc::now().timestamp_millis();
        let lock_until = now_ms.saturating_add(lock_duration_ms);

        let row = conn
            .query_row(
                // Drain forward, don't widen. Most-downstream kinds run first so
                // a slow LLM-bound `extract_chunk` can't starve the seal pipeline
                // behind it.
                "UPDATE mem_tree_jobs
                    SET status = 'running',
                        attempts = attempts + 1,
                        started_at_ms = ?1,
                        locked_until_ms = ?2,
                        last_error = NULL
                  WHERE id = (
                      SELECT id FROM mem_tree_jobs
                       WHERE status = 'ready'
                         AND available_at_ms <= ?1
                         AND kind NOT IN ('topic_route', 'digest_daily')
                       ORDER BY
                         CASE kind
                           WHEN 'seal'          THEN 1
                           WHEN 'flush_stale'   THEN 2
                           WHEN 'append_buffer' THEN 3
                           ELSE 4
                         END ASC,
                         available_at_ms ASC
                       LIMIT 1
                  )
              RETURNING id, kind, payload_json, dedupe_key, status, attempts,
                        max_attempts, available_at_ms, locked_until_ms, last_error,
                        created_at_ms, started_at_ms, completed_at_ms,
                        failure_reason, failure_class",
                params![now_ms, lock_until],
                row_to_job,
            )
            .optional()
            .context("Failed to claim next mem_tree_jobs row")?;
        Ok(row)
    })
}

/// Delete any retired-kind rows (`topic_route`, `digest_daily`) left over from
/// before the global/topic trees were removed. The chunks schema is shared DDL
/// and never migrated by this crate, so this is the queue's own one-shot
/// cleanup; callers run it at worker startup. Returns the number deleted.
pub fn purge_retired_jobs(config: &MemoryConfig) -> Result<usize> {
    with_connection(config, |conn| {
        let n = conn.execute(
            "DELETE FROM mem_tree_jobs WHERE kind IN ('topic_route', 'digest_daily')",
            [],
        )?;
        Ok(n)
    })
}

/// Quick count helper for tests / diagnostics.
pub fn count_by_status(config: &MemoryConfig, status: JobStatus) -> Result<u64> {
    with_connection(config, |conn| {
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM mem_tree_jobs WHERE status = ?1",
            params![status.as_str()],
            |r| r.get(0),
        )?;
        Ok(n.max(0) as u64)
    })
}

/// Count terminally-`failed` jobs whose typed class is `unrecoverable` — the
/// ones [`super::store_settle::requeue_transient_failed`] deliberately leaves
/// parked (budget / auth / dim-mismatch) because retrying can't help. The
/// predicate is the exact complement of the requeue gate's
/// `IS NULL OR != 'unrecoverable'`, so the two never drift.
pub fn count_failed_unrecoverable(config: &MemoryConfig) -> Result<u64> {
    with_connection(config, |conn| {
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM mem_tree_jobs \
              WHERE status = 'failed' AND failure_class = 'unrecoverable'",
            [],
            |r| r.get(0),
        )?;
        Ok(n.max(0) as u64)
    })
}

/// Total count regardless of status — handy for assertions.
pub fn count_total(config: &MemoryConfig) -> Result<u64> {
    with_connection(config, |conn| {
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM mem_tree_jobs", [], |r| r.get(0))?;
        Ok(n.max(0) as u64)
    })
}

/// Fetch one job by id (test/diagnostic helper).
pub fn get_job(config: &MemoryConfig, id: &str) -> Result<Option<Job>> {
    with_connection(config, |conn| {
        let job = conn
            .query_row(
                "SELECT id, kind, payload_json, dedupe_key, status, attempts, max_attempts,
                        available_at_ms, locked_until_ms, last_error,
                        created_at_ms, started_at_ms, completed_at_ms,
                        failure_reason, failure_class
                   FROM mem_tree_jobs WHERE id = ?1",
                params![id],
                row_to_job,
            )
            .optional()?;
        Ok(job)
    })
}

/// Map one `mem_tree_jobs` row (in the fixed column order used by every query
/// in this module: `id, kind, payload_json, dedupe_key, status, attempts,
/// max_attempts, available_at_ms, locked_until_ms, last_error, created_at_ms,
/// started_at_ms, completed_at_ms, failure_reason, failure_class`) into a
/// [`Job`]. Negative `attempts`/`max_attempts` (which should never occur) are
/// clamped to 0 rather than panicking. Returns `Err` if `kind` or `status`
/// fails to parse — callers should treat that as a schema/data invariant
/// violation, not a transient error.
pub(crate) fn row_to_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<Job> {
    let id: String = row.get(0)?;
    let kind_s: String = row.get(1)?;
    let payload_json: String = row.get(2)?;
    let dedupe_key: Option<String> = row.get(3)?;
    let status_s: String = row.get(4)?;
    let attempts: i64 = row.get(5)?;
    let max_attempts: i64 = row.get(6)?;
    let available_at_ms: i64 = row.get(7)?;
    let locked_until_ms: Option<i64> = row.get(8)?;
    let last_error: Option<String> = row.get(9)?;
    let created_at_ms: i64 = row.get(10)?;
    let started_at_ms: Option<i64> = row.get(11)?;
    let completed_at_ms: Option<i64> = row.get(12)?;
    let failure_reason: Option<String> = row.get(13)?;
    let failure_class: Option<String> = row.get(14)?;

    let kind = JobKind::parse(&kind_s).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, e.into())
    })?;
    let status = JobStatus::parse(&status_s).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, e.into())
    })?;

    Ok(Job {
        id,
        kind,
        payload_json,
        dedupe_key,
        status,
        attempts: attempts.max(0) as u32,
        max_attempts: max_attempts.max(0) as u32,
        available_at_ms,
        locked_until_ms,
        last_error,
        failure_reason,
        failure_class,
        created_at_ms,
        started_at_ms,
        completed_at_ms,
    })
}

/// Exponential backoff: attempt 1 → 60s, 2 → 120s, 3 → 240s, capped at 1h.
pub(crate) fn backoff_ms(attempts_so_far: u32) -> i64 {
    // `attempts_so_far` is the count BEFORE the next retry's attempt — so the
    // first retry uses `attempts_so_far == 1`, giving base * 2^0 = 60s.
    let exp = attempts_so_far.saturating_sub(1).min(20);
    let mult = 1i64 << exp;
    let raw = RETRY_BASE_MS.saturating_mul(mult);
    raw.min(RETRY_CAP_MS)
}

/// True if `kind` is a retired wire string (used by tests / callers that want
/// to recognise a raw row's kind without parsing it into [`JobKind`]).
pub fn is_retired_kind(kind: &str) -> bool {
    RETIRED_JOB_KINDS.contains(&kind)
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
