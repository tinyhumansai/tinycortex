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

use std::sync::LazyLock;

use anyhow::Result;

use crate::memory::config::MemoryConfig;
use crate::memory::queue::gate::{LlmGate, Permit};
use crate::memory::queue::handlers::{self, QueueDelegates};
use crate::memory::queue::store::{claim_next, purge_retired_jobs, DEFAULT_LOCK_DURATION_MS};
use crate::memory::queue::store_settle::{
    mark_deferred, mark_done, mark_failed_typed, recover_stale_locks,
};
use crate::memory::queue::types::{Job, JobFailure, JobOutcome};

/// Process-wide LLM concurrency gate. Single-slot by default (mirrors the
/// upstream single-permit semaphore); LLM-bound jobs hold a permit for the
/// duration of their handler.
static LLM_GATE: LazyLock<LlmGate> = LazyLock::new(LlmGate::default);

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
pub async fn run_once(config: &MemoryConfig, delegates: &dyn QueueDelegates) -> Result<bool> {
    let Some(job) = claim_next(config, DEFAULT_LOCK_DURATION_MS)? else {
        return Ok(false);
    };

    // LLM-bound jobs hold a permit from the global gate for the lifetime of the
    // handler; non-LLM jobs (AppendBuffer, FlushStale) run without one.
    let permit: Option<Permit> = if job.kind.is_llm_bound() {
        Some(LLM_GATE.acquire())
    } else {
        None
    };

    let result = handlers::handle_job(config, &job, delegates).await;
    drop(permit);

    settle_job(config, &job, result)?;
    Ok(true)
}

fn settle_job(config: &MemoryConfig, job: &Job, result: Result<JobOutcome>) -> Result<()> {
    match result {
        Ok(JobOutcome::Done) => mark_done(config, job),
        Ok(JobOutcome::Defer { until_ms, reason }) => {
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
            mark_failed_typed(config, job, &message, typed)
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

#[cfg(test)]
#[path = "worker_tests.rs"]
mod tests;
