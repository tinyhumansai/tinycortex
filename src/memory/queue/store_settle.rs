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

/// Maximum lifetime of a repeatedly deferred job. Deferral is intentionally
/// free of the handler failure budget, but it must still have a terminal bound.
#[cfg(test)]
pub(crate) const MAX_DEFER_AGE_MS: i64 = 7 * 24 * 60 * 60 * 1_000;

/// Mark a claimed job as `done`. Clears the lock and stamps `completed_at_ms`.
pub fn mark_done(config: &MemoryConfig, job: &Job) -> Result<()> {
    mark_done_with_followups(config, job, &[])
}

/// Mark a claimed job done and enqueue all of its follow-up jobs in the same
/// SQLite transaction. A crash therefore exposes either neither transition or
/// both, never a completed parent with a missing child edge.
pub(crate) fn mark_done_with_followups(
    config: &MemoryConfig,
    job: &Job,
    follow_ups: &[NewJob],
) -> Result<()> {
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
                crate::memory::queue::store::enqueue_tx(&tx, follow_up)?;
            }
        }
        tx.commit()?;
        Ok(())
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

fn requeue_failed_where(config: &MemoryConfig, predicate: &str) -> Result<u64> {
    with_connection(config, |conn| {
        let now_ms = Utc::now().timestamp_millis();
        let sql = format!(
            "UPDATE mem_tree_jobs
                SET status = 'ready',
                    attempts = 0,
                    available_at_ms = ?1,
                    locked_until_ms = NULL,
                    started_at_ms = NULL,
                    completed_at_ms = NULL,
                    last_error = NULL,
                    failure_reason = NULL,
                    failure_class = NULL
              WHERE {predicate}"
        );
        let n = conn.execute(&sql, params![now_ms])?;
        Ok(n as u64)
    })
}

#[cfg(test)]
#[path = "store_settle_tests.rs"]
mod tests;
