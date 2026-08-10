//! SQLite job-queue settlement: `mark_done` / `mark_failed` / `mark_deferred`,
//! stale-lock recovery, graceful-shutdown release, and the requeue helpers.
//!
//! Split out of [`super::store`] to keep each file under the source-size cap.
//! Every settle is gated on the claim token (`attempts` + `started_at_ms`
//! matching the [`claim_next`](super::store::claim_next) snapshot) so a stale
//! worker — one whose lease expired and whose row was re-claimed — cannot
//! clobber the new lessee: `rows_affected == 0` is a silent no-op.

use anyhow::Result;
use chrono::Utc;
use rusqlite::params;

use crate::memory::chunks::with_connection;
use crate::memory::config::MemoryConfig;
use crate::memory::queue::store::backoff_ms_with_policy;
use crate::memory::queue::types::{Job, JobFailure, NewJob};

/// Mark a claimed job as `done`. Clears the lock and stamps `completed_at_ms`.
pub fn mark_done(config: &MemoryConfig, job: &Job) -> Result<()> {
    mark_done_with_followups(config, job, &[]).map(|_| ())
}

/// Mark a claimed job done and enqueue all of its follow-up jobs in the same
/// SQLite transaction. A crash therefore exposes either neither transition or
/// both, never a completed parent with a missing child edge.
pub(crate) fn mark_done_with_followups(
    config: &MemoryConfig,
    job: &Job,
    follow_ups: &[NewJob],
) -> Result<bool> {
    let job_id = &job.id;
    let claim_attempts = job.attempts as i64;
    let claim_started_at = job.started_at_ms;
    with_connection(config, |conn| {
        let now_ms = Utc::now().timestamp_millis();
        let tx = conn.unchecked_transaction()?;
        let updated = tx.execute(
            "UPDATE mem_tree_jobs
                SET status = 'done',
                    completed_at_ms = ?1,
                    locked_until_ms = NULL,
                    last_error = NULL
              WHERE id = ?2
                AND attempts = ?3
                AND started_at_ms IS ?4",
            params![now_ms, job_id, claim_attempts, claim_started_at],
        )?;
        if updated != 0 {
            for follow_up in follow_ups {
                crate::memory::queue::store::enqueue_tx_with_default(
                    &tx,
                    follow_up,
                    config.queue.max_attempts,
                )?;
            }
        }
        tx.commit()?;
        Ok(updated != 0)
    })
}

/// Settle a failed job. If `attempts < max_attempts`, the row goes back to
/// `ready` with an exponential-backoff `available_at_ms`; otherwise it
/// terminates as `failed`. Either way `last_error` is recorded.
pub fn mark_failed(config: &MemoryConfig, job: &Job, error: &str) -> Result<()> {
    mark_failed_typed(config, job, error, None)
}

/// Like [`mark_failed`], but with an optional typed [`JobFailure`]
/// classification. When `failure` is `Some` and **unrecoverable** the job
/// terminates as `failed` **immediately** — no retry budget is burned, since
/// retrying the same input cannot succeed — and the typed `failure_reason` /
/// `failure_class` columns are persisted alongside the freeform `last_error`.
/// Transient classifications (and the untyped `None` case) keep the existing
/// attempts-bounded retry-with-backoff behaviour.
pub fn mark_failed_typed(
    config: &MemoryConfig,
    job: &Job,
    error: &str,
    failure: Option<&JobFailure>,
) -> Result<()> {
    let job_id = &job.id;
    let attempts = job.attempts as i64;
    let max_attempts = job.max_attempts as i64;
    let claim_started_at = job.started_at_ms;
    let unrecoverable = failure.map(|f| f.is_unrecoverable()).unwrap_or(false);
    let failure_reason = failure.map(|f| f.code);
    let failure_class = failure.map(|f| f.class);
    with_connection(config, |conn| {
        let now_ms = Utc::now().timestamp_millis();

        // Terminal when the retry budget is exhausted OR the failure is
        // classified unrecoverable (fail fast).
        if attempts >= max_attempts || unrecoverable {
            conn.execute(
                "UPDATE mem_tree_jobs
                    SET status = 'failed',
                        completed_at_ms = ?1,
                        locked_until_ms = NULL,
                        last_error = ?2,
                        failure_reason = ?6,
                        failure_class = ?7
                  WHERE id = ?3
                    AND attempts = ?4
                    AND started_at_ms IS ?5",
                params![
                    now_ms,
                    error,
                    job_id,
                    attempts,
                    claim_started_at,
                    failure_reason,
                    failure_class,
                ],
            )?;
        } else {
            let next_at = now_ms.saturating_add(backoff_ms_with_policy(
                attempts as u32,
                config.queue.retry_base_ms,
                config.queue.retry_cap_ms,
            ));
            conn.execute(
                "UPDATE mem_tree_jobs
                    SET status = 'ready',
                        available_at_ms = ?1,
                        locked_until_ms = NULL,
                        last_error = ?2
                  WHERE id = ?3
                    AND attempts = ?4
                    AND started_at_ms IS ?5",
                params![next_at, error, job_id, attempts, claim_started_at],
            )?;
        }
        Ok(())
    })
}

/// Mark a claimed job as deferred: put it back to `ready` with
/// `available_at_ms = until_ms` so [`claim_next`](super::store::claim_next)
/// re-picks it once the wake-up time has passed. The handler ran successfully
/// but chose not to make progress, so this path **does not** burn the failure
/// budget — the `attempts` bump that the claim applied is reverted.
///
/// `reason` is recorded in `last_error` for visibility and `started_at_ms` is
/// cleared so the next claim stamps a fresh start time.
///
/// Deferral remains free of the attempt budget, but a row that has spent seven
/// days in the defer cycle is parked as an unrecoverable failure. This bounds
/// permanently non-progressing handlers without penalising ordinary
/// rate-limit or multi-batch backfill deferrals.
pub fn mark_deferred(config: &MemoryConfig, job: &Job, until_ms: i64, reason: &str) -> Result<()> {
    let job_id = &job.id;
    let claim_attempts = job.attempts as i64;
    let pre_claim_attempts = claim_attempts.saturating_sub(1);
    let claim_started_at = job.started_at_ms;
    with_connection(config, |conn| {
        let now_ms = Utc::now().timestamp_millis();
        let oldest_allowed_ms = now_ms.saturating_sub(config.queue.max_defer_age_ms);
        conn.execute(
            "UPDATE mem_tree_jobs
                SET status = CASE WHEN created_at_ms <= ?2 THEN 'failed' ELSE 'ready' END,
                    attempts = ?3,
                    available_at_ms = ?4,
                    locked_until_ms = NULL,
                    started_at_ms = NULL,
                    completed_at_ms = CASE WHEN created_at_ms <= ?2 THEN ?1 ELSE NULL END,
                    last_error = ?5,
                    failure_reason = CASE WHEN created_at_ms <= ?2 THEN 'defer_age_exceeded' ELSE NULL END,
                    failure_class = CASE WHEN created_at_ms <= ?2 THEN 'unrecoverable' ELSE NULL END
              WHERE id = ?6
                AND attempts = ?7
                AND started_at_ms IS ?8",
            params![
                now_ms,
                oldest_allowed_ms,
                pre_claim_attempts,
                until_ms,
                reason,
                job_id,
                claim_attempts,
                claim_started_at,
            ],
        )?;
        Ok(())
    })
}

/// Flip any `running` row whose `locked_until_ms` has expired back to `ready`.
/// Called once at worker startup so a process crash mid-job doesn't leave work
/// stranded. Returns the number of rows recovered.
pub fn recover_stale_locks(config: &MemoryConfig) -> Result<usize> {
    with_connection(config, |conn| {
        let now_ms = Utc::now().timestamp_millis();
        let n = conn.execute(
            "UPDATE mem_tree_jobs
                SET status = 'ready',
                    last_error = COALESCE(last_error, 'recovered_from_stale_lock')
              WHERE status = 'running'
                AND locked_until_ms IS NOT NULL
                AND locked_until_ms < ?1",
            params![now_ms],
        )?;
        Ok(n)
    })
}

/// Release this process's in-flight job locks on a *graceful* shutdown: flip
/// every `running` row back to `ready` so the work is immediately re-claimable
/// on next launch instead of waiting out the lease. The core runs a single
/// worker pool, so any `running` row at clean-shutdown time was claimed by us.
/// Returns the number of rows released.
///
/// This reverts the `attempts` bump applied by
/// [`claim_next`](super::store::claim_next): a graceful interruption is not a
/// handler failure and must not consume the job's retry budget.
pub fn release_running_locks(config: &MemoryConfig) -> Result<usize> {
    with_connection(config, |conn| {
        let n = conn.execute(
            "UPDATE mem_tree_jobs
                SET status = 'ready',
                    attempts = CASE WHEN attempts > 0 THEN attempts - 1 ELSE 0 END,
                    locked_until_ms = NULL,
                    started_at_ms = NULL
              WHERE status = 'running'",
            [],
        )?;
        Ok(n)
    })
}

/// Requeue every terminally-`failed` job back to `ready`. Resets `attempts` to
/// 0 (a fresh retry budget), clears the typed `failure_reason` /
/// `failure_class` and `last_error`, and makes the row immediately available.
/// Returns the number of jobs requeued. Backs the manual "retry failed" action.
pub fn requeue_failed(config: &MemoryConfig) -> Result<u64> {
    requeue_failed_where(config, "status = 'failed'")
}

/// Requeue only failed jobs whose recorded failure class is NOT
/// `unrecoverable` — i.e. transient failures (network 5xx, timeouts,
/// SQLITE_BUSY) and legacy rows with no class recorded. The automatic
/// self-healing variant: unrecoverable failures stay parked for the manual
/// retry path so a bad config can't retry-loop forever.
pub fn requeue_transient_failed(config: &MemoryConfig) -> Result<u64> {
    requeue_failed_where(
        config,
        "status = 'failed' AND (failure_class IS NULL OR failure_class != 'unrecoverable')",
    )
}

/// Reset all terminally-failed jobs back to `ready`. Alias kept for parity with
/// OpenHuman's `retry_all_failed` RPC entry point.
pub fn retry_all_failed(config: &MemoryConfig) -> Result<u64> {
    requeue_failed(config)
}

/// Shared requeue worker for [`requeue_failed`] / [`requeue_transient_failed`].
///
/// # Why this is not one blanket `UPDATE`
///
/// `idx_mem_tree_jobs_dedupe_active` is a **partial unique index** over
/// `dedupe_key` restricted to `status IN ('ready', 'running')`. Failed rows sit
/// outside it, so the same `dedupe_key` can legitimately accumulate several
/// `failed` rows over time (the same seal / re-embed unit of work failing on
/// successive attempts). The moment a single `UPDATE` flips two such siblings to
/// `ready` they both enter the index and collide, SQLite aborts the **whole**
/// statement, and **nothing** is requeued.
///
/// That is not hypothetical: it is what the periodic self-heal hit in
/// production, logging `UNIQUE constraint failed: mem_tree_jobs.dedupe_key`
/// every three hours and on every boot while the queue stayed permanently
/// parked — and the manual "retry failed" path aborted the same way, so the
/// user had no way out either.
///
/// So the requeue is filtered before it is applied:
///
/// - a key that **already has an active row** (`ready`/`running`) is skipped
///   entirely — live work already covers it, and requeueing would collide;
/// - among matched failed rows **sharing a key**, only the newest is requeued;
///   its siblings are settled as `cancelled` (superseded duplicates of the same
///   unit of work) so they stop being counted as failures the user must act on.
/// - rows with `dedupe_key IS NULL` are outside the index and always requeue.
///
/// Selection and mutation run in one transaction, so a concurrent claim can't
/// slip an active row in between the read and the write.
///
/// Returns the number of jobs actually flipped back to `ready` (cancelled
/// duplicates are not counted — they were not requeued).
fn requeue_failed_where(config: &MemoryConfig, predicate: &str) -> Result<u64> {
    with_connection(config, |conn| {
        let now_ms = Utc::now().timestamp_millis();
        let tx = conn.unchecked_transaction()?;

        // Newest-first so the first row seen for a key is the one to keep.
        // `completed_at_ms` is stamped on failure; fall back to `created_at_ms`
        // for legacy rows that predate the column. `created_at_ms` then `id`
        // break exact-timestamp ties by enqueue order — job ids are random
        // UUIDs, so `id` alone carries no ordering signal. For rows sharing a
        // `dedupe_key` the pick is immaterial to correctness anyway (they are
        // the same unit of work), but a stable order keeps the choice
        // reproducible.
        let select = format!(
            "SELECT id, dedupe_key FROM mem_tree_jobs
              WHERE {predicate}
              ORDER BY COALESCE(completed_at_ms, created_at_ms) DESC, created_at_ms DESC, id DESC"
        );
        let candidates: Vec<(String, Option<String>)> = {
            let mut stmt = tx.prepare(&select)?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        if candidates.is_empty() {
            return Ok(0);
        }

        // Keys already held by a live row: requeueing onto one of these is the
        // collision case, and the live row is already doing the work.
        let active_keys: std::collections::HashSet<String> = {
            let mut stmt = tx.prepare(
                "SELECT DISTINCT dedupe_key FROM mem_tree_jobs
                  WHERE dedupe_key IS NOT NULL AND status IN ('ready', 'running')",
            )?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            rows.collect::<rusqlite::Result<std::collections::HashSet<_>>>()?
        };

        let mut claimed_keys: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut to_requeue: Vec<String> = Vec::new();
        let mut to_cancel: Vec<String> = Vec::new();
        let mut skipped_active = 0_usize;

        for (id, dedupe_key) in candidates {
            match dedupe_key {
                // Outside the partial unique index — never collides.
                None => to_requeue.push(id),
                Some(key) if active_keys.contains(&key) => skipped_active += 1,
                // First (newest) row for this key wins the requeue; every later
                // sibling is the same unit of work, so settle it rather than
                // leaving it counted as a failure the user must act on.
                Some(key) => {
                    if claimed_keys.insert(key) {
                        to_requeue.push(id);
                    } else {
                        to_cancel.push(id);
                    }
                }
            }
        }

        for id in &to_cancel {
            tx.execute(
                "UPDATE mem_tree_jobs
                    SET status = 'cancelled',
                        locked_until_ms = NULL,
                        completed_at_ms = ?2,
                        last_error = 'superseded by a newer job with the same dedupe_key'
                  WHERE id = ?1",
                params![id, now_ms],
            )?;
        }

        let mut requeued = 0_u64;
        for id in &to_requeue {
            requeued += tx.execute(
                "UPDATE mem_tree_jobs
                    SET status = 'ready',
                        attempts = 0,
                        available_at_ms = ?2,
                        locked_until_ms = NULL,
                        started_at_ms = NULL,
                        completed_at_ms = NULL,
                        last_error = NULL,
                        failure_reason = NULL,
                        failure_class = NULL
                  WHERE id = ?1",
                params![id, now_ms],
            )? as u64;
        }

        tx.commit()?;

        if !to_cancel.is_empty() || skipped_active > 0 {
            log::debug!(
                "[queue::requeue] action=requeue_failed requeued={requeued} \
                 cancelled_duplicates={} skipped_active_key={skipped_active}",
                to_cancel.len()
            );
        }

        Ok(requeued)
    })
}

#[cfg(test)]
#[path = "store_settle_tests.rs"]
mod tests;
