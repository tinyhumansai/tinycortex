//! Worker driver: claim one job, run its handler under the LLM gate, settle.
//!
//! OpenHuman spawned a `tokio` worker pool plus a wall-clock scheduler here.
//! `tokio` is a dev-only dependency in this crate and the runtime/spawn plumbing
//! (graceful-shutdown hooks, Sentry reporting, the corrupt-DB quarantine path)
//! is out of the ported surface, so the durable primitive is [`run_once`]: a
//! single claim→handle→settle step that a host drives in its own loop (and that
//! [`drain_until_idle`](crate::memory::queue::drain_until_idle) calls to settle
//! the queue deterministically in tests).
//!
//! The SQLite error classifiers ([`is_sqlite_busy`] et al.) are ported verbatim
//! so a host loop can reproduce OpenHuman's "back off, don't page" policy for
//! transient write-lock / I/O / disk-full / corruption conditions.
//! [`is_host_io_error`] extends that family to persistent **host-filesystem**
//! failures (EIO/ENOSPC/EROFS — a dying SD card or full/read-only mount) that
//! surface as `std::io::Error` rather than a SQLite code; a host loop is meant
//! to use it to back off long and page once instead of flooding (Sentry
//! CORE-RUST-19J). The `tokio`-feature runtime loop's `backoff_for`
//! (`memory::queue::runtime`) includes this classifier.
//! The Sentry-once emission and the storage-degraded flag stay host-owned
//! regardless.

use std::sync::LazyLock;

use anyhow::Result;

use crate::memory::config::MemoryConfig;
use crate::memory::queue::gate::{LlmGate, Permit};
use crate::memory::queue::handlers::{self, QueueDelegates};
use crate::memory::queue::ops::set_backfill_in_progress;
use crate::memory::queue::store::{claim_next, purge_retired_jobs};
use crate::memory::queue::store_settle::{
    mark_deferred, mark_done_with_followups, mark_failed_typed, recover_stale_locks,
};
use crate::memory::queue::types::{
    Job, JobFailure, JobKind, JobOutcome, JobStatus, NewJob, SealPayload,
};

/// Process-wide LLM concurrency gate. Single-slot by default (mirrors the
/// upstream single-permit semaphore); LLM-bound jobs hold a permit for the
/// duration of their handler.
static LLM_GATE: LazyLock<LlmGate> = LazyLock::new(LlmGate::default);

/// Short delay used when an LLM job is claimed while the process-wide gate is
/// full. Deferring releases the job lease and attempt immediately, avoiding a
/// blocking wait on an async executor thread.
const LLM_GATE_RETRY_MS: i64 = 50;

/// The global LLM gate (exposed so hosts/tests can inspect or share it).
pub fn llm_gate() -> &'static LlmGate {
    &LLM_GATE
}

/// Startup housekeeping: purge retired-kind rows and recover any leases left
/// `running` by a previous hard kill. Returns `(purged, recovered)` counts.
/// A host calls this once before entering its `run_once` loop.
pub fn bootstrap(config: &MemoryConfig) -> Result<(usize, usize)> {
    let purged = purge_retired_jobs(config)?;
    let recovered = recover_stale_locks(config)?;
    Ok((purged, recovered))
}

/// Claim and run a single job. Returns `true` when work was processed, `false`
/// when no eligible row was available.
///
/// The job's configured lease starts counting at claim
/// time, before the LLM gate is acquired below — a job that waits a long time
/// for a free permit eats into its own lease window before its handler even
/// starts.
///
/// See [`LlmGate::acquire`] for the blocking-wait caveat: this function calls
/// it from inside an `async fn`, which is the exact pattern that caveat warns
/// about. On a `current_thread` runtime, two concurrent `run_once` calls can
/// deadlock (one holds the only permit and needs the executor to finish its
/// handler and drop it; the other blocks the only executor thread inside
/// `acquire`).
pub async fn run_once(config: &MemoryConfig, delegates: &dyn QueueDelegates) -> Result<bool> {
    run_once_with_gate(config, delegates, &LLM_GATE).await
}

async fn run_once_with_gate(
    config: &MemoryConfig,
    delegates: &dyn QueueDelegates,
    gate: &LlmGate,
) -> Result<bool> {
    let Some(job) = claim_next(config, config.queue.lock_duration_ms)? else {
        return Ok(false);
    };

    // LLM-bound jobs hold a permit from the global gate for the lifetime of the
    // handler; non-LLM jobs (AppendBuffer, FlushStale) run without one.
    let permit: Option<Permit> = if job.kind.is_llm_bound() {
        let Some(permit) = gate.try_acquire() else {
            mark_deferred(
                config,
                &job,
                chrono::Utc::now().timestamp_millis() + LLM_GATE_RETRY_MS,
                "llm concurrency gate busy",
            )?;
            return Ok(true);
        };
        Some(permit)
    } else {
        None
    };

    let result = handlers::plan_job(config, &job, delegates).await;
    drop(permit);

    settle_planned_job(config, &job, result)?;
    Ok(true)
}

/// Translate a handler plan into the matching store settlement call. Successful
/// follow-up enqueues commit in the same transaction as `mark_done`.
/// Errors from the settlement call itself (e.g. a busy database) propagate to
/// the caller — the job's `running` row is then left for lease-expiry
/// recovery rather than being retried inline here.
#[cfg(test)]
fn settle_job(config: &MemoryConfig, job: &Job, result: Result<JobOutcome>) -> Result<()> {
    settle_planned_job(
        config,
        job,
        result.map(|outcome| handlers::JobPlan {
            outcome,
            follow_ups: Vec::new(),
        }),
    )
}

fn settle_planned_job(
    config: &MemoryConfig,
    job: &Job,
    result: Result<handlers::JobPlan>,
) -> Result<()> {
    match result {
        Ok(handlers::JobPlan {
            outcome: JobOutcome::Done,
            follow_ups,
        }) => {
            let arms_reembed = follow_ups
                .iter()
                .any(|follow_up| follow_up.kind == JobKind::ReembedBackfill);
            mark_done_with_followups(config, job, &follow_ups)?;
            if arms_reembed {
                set_backfill_in_progress(true);
            }

            // A same-level seal enqueue can be suppressed while this row is
            // running. Once its dedupe key is released, re-check the live
            // buffer and restore the edge if content appended during the seal
            // crossed the gate again.
            if job.kind == JobKind::Seal
                && matches!(
                    crate::memory::queue::store::get_job(config, &job.id)?,
                    Some(current) if current.status == JobStatus::Done
                )
            {
                let payload: SealPayload = serde_json::from_str(&job.payload_json)?;
                let buffer = crate::memory::tree::store::get_buffer(
                    config,
                    &payload.tree_id,
                    payload.level,
                )?;
                if crate::memory::tree::should_seal(config, &buffer) {
                    crate::memory::queue::store::enqueue(config, &NewJob::seal(&payload)?)?;
                }
            }
            Ok(())
        }
        Ok(handlers::JobPlan {
            outcome: JobOutcome::Defer { until_ms, reason },
            ..
        }) => {
            // Defer is normal operation (transient blocker) — does NOT burn the
            // failure budget; `mark_deferred` reverts the claim's attempts bump.
            mark_deferred(config, job, until_ms, &reason)
        }
        Err(err) => {
            // Preserve the full anyhow cause chain in `last_error` so a reader
            // can see the root cause. If the chain carries a typed `JobFailure`,
            // pass it through so an unrecoverable cause fails fast instead of
            // burning the retry budget.
            let message = format!("{err:#}");
            let typed = err.downcast_ref::<JobFailure>();
            mark_failed_typed(config, job, &message, typed)?;

            // The handler clears this flag on every successful terminal
            // outcome. Mirror that cleanup when a re-embed row exhausts its
            // budget (or fails fast), otherwise retrieval remains in the
            // process-wide "backfill pending" mode indefinitely.
            if job.kind == JobKind::ReembedBackfill
                && matches!(
                    crate::memory::queue::store::get_job(config, &job.id)?,
                    Some(current) if current.status == JobStatus::Failed
                )
            {
                set_backfill_in_progress(false);
            }
            Ok(())
        }
    }
}

/// Classify whether an error is transient SQLite write-lock contention
/// (`SQLITE_BUSY` or `SQLITE_LOCKED`): back off and re-poll, don't escalate.
pub fn is_sqlite_busy(err: &anyhow::Error) -> bool {
    if let Some(rusqlite::Error::SqliteFailure(sqlite_err, _)) =
        err.downcast_ref::<rusqlite::Error>()
    {
        return matches!(
            sqlite_err.code,
            rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
        );
    }
    let msg = format!("{err:#}").to_ascii_lowercase();
    msg.contains("database is locked") || msg.contains("database table is locked")
}

/// Classify whether an error is a transient I/O failure that should be silently
/// backed off (WAL truncate, the `-shm` cold-start family, `CANTOPEN`, the
/// connection circuit breaker).
pub fn is_sqlite_io_transient(err: &anyhow::Error) -> bool {
    if let Some(rusqlite::Error::SqliteFailure(f, _)) = err.downcast_ref::<rusqlite::Error>() {
        // 14 CANTOPEN, 1546 TRUNCATE, 4618 SHMOPEN, 4874 SHMSIZE, 5386 SHMMAP,
        // 8714 IN_PAGE.
        if matches!(f.extended_code, 14 | 1546 | 4618 | 4874 | 5386 | 8714) {
            return true;
        }
        if f.code == rusqlite::ErrorCode::CannotOpen {
            return true;
        }
    }
    let msg = format!("{err:#}").to_ascii_lowercase();
    msg.contains("circuit breaker open")
        || msg.contains("disk i/o error")
        || msg.contains("unable to open database file")
        || msg.contains("xshmmap")
        || msg.contains("truncate file")
}

/// Classify `SQLITE_FULL` (disk full): a persistent host condition — back off
/// long and stay silent until the user frees space.
pub fn is_sqlite_disk_full(err: &anyhow::Error) -> bool {
    if let Some(rusqlite::Error::SqliteFailure(sqlite_err, _)) =
        err.downcast_ref::<rusqlite::Error>()
    {
        if sqlite_err.code == rusqlite::ErrorCode::DiskFull {
            return true;
        }
    }
    let msg = format!("{err:#}").to_ascii_lowercase();
    msg.contains("database or disk is full")
        || msg.contains("insertion failed because database is full")
}

/// Classify `SQLITE_CORRUPT` / `SQLITE_NOTADB`: persistent on-disk damage that
/// never clears on its own — a host should quarantine + rebuild, not re-poll.
pub fn is_sqlite_corrupt(err: &anyhow::Error) -> bool {
    if let Some(rusqlite::Error::SqliteFailure(sqlite_err, _)) =
        err.downcast_ref::<rusqlite::Error>()
    {
        if matches!(
            sqlite_err.code,
            rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase
        ) {
            return true;
        }
    }
    let msg = format!("{err:#}").to_ascii_lowercase();
    msg.contains("database disk image is malformed") || msg.contains("file is not a database")
}

/// Classify a persistent **host-filesystem** I/O failure (EIO `5`, ENOSPC `28`,
/// EROFS `30`) surfaced as a `std::io::Error` — a dying SD card, a full disk, or
/// a kernel-remounted-read-only mount. These are user-only-fixable and never
/// clear on their own, so a host loop should back off long and page **once**
/// rather than re-poll and flood (Sentry CORE-RUST-19J).
///
/// Distinct from [`is_sqlite_disk_full`]: `SQLITE_FULL` arrives as a SQLite code
/// and stays in that arm; this family is the raw OS error bubbling out of
/// `create_dir_all` / `File` operations, often through anyhow context layers,
/// so both the typed downcast and the flattened `(os error N)` text are checked.
/// EACCES (`13`) and ENOENT (`2`) are deliberately excluded — those are genuine
/// bugs that must keep reporting.
pub fn is_host_io_error(err: &anyhow::Error) -> bool {
    if let Some(io_err) = err.downcast_ref::<std::io::Error>() {
        if matches!(io_err.raw_os_error(), Some(5) | Some(28) | Some(30)) {
            return true;
        }
    }
    let msg = format!("{err:#}").to_ascii_lowercase();
    msg.contains("(os error 5)") || msg.contains("(os error 28)") || msg.contains("(os error 30)")
}

#[cfg(test)]
#[path = "worker_tests.rs"]
mod tests;
