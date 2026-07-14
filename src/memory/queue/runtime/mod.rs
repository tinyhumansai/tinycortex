//! Always-on async worker + scheduler loops (feature `tokio`).
//!
//! The dependency-light core exposes only host-driven primitives — [`run_once`]
//! (claim → handle → settle one job) and the plain [`scheduler`] functions — so
//! it stays synchronous and free of an async runtime. This module, gated behind
//! the `tokio` feature, wires those primitives into cancellable background
//! loops for hosts that want the append/seal/flush pipeline to run on its own.
//!
//! ## Loops
//!
//! - `run_worker`: repeatedly runs one job, sleeping between polls. Idle polls
//!   use the configured backoff; transient SQLite errors back off by kind
//!   (busy / I/O / disk-full) and re-poll; corruption triggers database
//!   recovery and the loop continues polling.
//! - `run_scheduler`: on a fixed cadence, recover expired leases, enqueue
//!   `flush_stale` (dedupe-safe), and `self_heal` transiently-failed jobs.
//! - `run`: bootstrap once, then drive both loops concurrently until
//!   shutdown or a fatal error.
//!
//! All loops observe a shared shutdown flag between jobs and during backoff,
//! so a triggered shutdown stops them promptly without aborting an in-flight
//! handler.
//!
//! [`run_once`]: crate::memory::queue::run_once
//! [`scheduler`]: crate::memory::queue::scheduler
//! [`bootstrap`]: crate::memory::queue::bootstrap

pub mod types;

use std::time::Duration;

use anyhow::{Context, Result};
use tokio::time::sleep;

use crate::memory::config::MemoryConfig;
use crate::memory::queue::handlers::QueueDelegates;
use crate::memory::queue::{scheduler, worker};

pub use types::{SchedulerLoopConfig, Shutdown, WorkerLoopConfig};

/// Longest single `sleep` slice used when waiting, so a triggered [`Shutdown`]
/// is observed within this bound even during a long backoff.
const SLEEP_SLICE: Duration = Duration::from_millis(200);

/// Run the worker loop until [`Shutdown`] is triggered or a fatal error occurs.
///
/// Each iteration claims and runs one job via [`run_once`](worker::run_once):
///
/// - work processed → yield and immediately poll again;
/// - no work → sleep [`WorkerLoopConfig::idle_backoff`];
/// - transient SQLite error → sleep the kind-specific backoff and re-poll;
/// - corruption (`SQLITE_CORRUPT`/`NOTADB`) → return the error so the host can
///   quarantine + rebuild rather than spin.
pub async fn run_worker(
    config: &MemoryConfig,
    delegates: &dyn QueueDelegates,
    opts: &WorkerLoopConfig,
    shutdown: &Shutdown,
) -> Result<()> {
    while !shutdown.is_triggered() {
        match worker::run_once(config, delegates).await {
            Ok(true) => {
                // Did work; give the runtime a chance to poll cooperatively,
                // then loop straight into the next claim.
                tokio::task::yield_now().await;
            }
            Ok(false) => interruptible_sleep(opts.idle_backoff, shutdown).await,
            Err(err) => {
                if worker::is_sqlite_corrupt(&err) {
                    crate::memory::chunks::recover_corrupt_db(config)
                        .context("recover corrupt queue database")?;
                    continue;
                }
                match backoff_for(&err, opts) {
                    Some(backoff) => interruptible_sleep(backoff, shutdown).await,
                    None => return Err(err),
                }
            }
        }
    }
    Ok(())
}

/// Run the scheduler loop until [`Shutdown`] is triggered or a fatal error.
///
/// Each tick enqueues a dedupe-suppressed `flush_stale` job and requeues
/// transiently-failed jobs. Transient errors are swallowed (the next tick
/// retries); corruption is returned to the host.
///
pub async fn run_scheduler(
    config: &MemoryConfig,
    opts: &SchedulerLoopConfig,
    shutdown: &Shutdown,
) -> Result<()> {
    while !shutdown.is_triggered() {
        if let Err(err) = crate::memory::queue::store_settle::recover_stale_locks(config) {
            if worker::is_sqlite_corrupt(&err) {
                crate::memory::chunks::recover_corrupt_db(config)
                    .context("recover corrupt queue database")?;
                continue;
            }
        }
        if let Err(err) = scheduler::enqueue_flush_stale(config) {
            if worker::is_sqlite_corrupt(&err) {
                crate::memory::chunks::recover_corrupt_db(config)
                    .context("recover corrupt queue database")?;
                continue;
            }
        }
        if let Err(err) = scheduler::self_heal(config) {
            if worker::is_sqlite_corrupt(&err) {
                crate::memory::chunks::recover_corrupt_db(config)
                    .context("recover corrupt queue database")?;
                continue;
            }
        }
        interruptible_sleep(opts.tick, shutdown).await;
    }
    Ok(())
}

/// Bootstrap the queue, then drive the worker and scheduler loops concurrently
/// until [`Shutdown`] is triggered or either loop hits a fatal error.
///
/// [`bootstrap`](worker::bootstrap) runs once up front (purge retired kinds,
/// recover leases left `running` by a previous hard kill). If either loop
/// returns an error, the other is cancelled and the error is propagated.
pub async fn run(
    config: &MemoryConfig,
    delegates: &dyn QueueDelegates,
    worker_opts: &WorkerLoopConfig,
    scheduler_opts: &SchedulerLoopConfig,
    shutdown: &Shutdown,
) -> Result<()> {
    worker::bootstrap(config)?;
    tokio::try_join!(
        run_worker(config, delegates, worker_opts, shutdown),
        run_scheduler(config, scheduler_opts, shutdown),
    )?;
    Ok(())
}

/// Choose a backoff for a `run_once` error, or `None` if it is fatal.
///
/// Classification order matters: corruption is checked first (fatal), then the
/// transient families in descending severity.
///
fn backoff_for(err: &anyhow::Error, opts: &WorkerLoopConfig) -> Option<Duration> {
    if worker::is_sqlite_corrupt(err) {
        return None;
    }
    if worker::is_host_io_error(err) {
        // Persistent host-filesystem failure (EIO/ENOSPC/EROFS surfaced as a raw
        // std::io::Error). User-only-fixable and never self-clears — back off
        // long, like disk-full, instead of hammering the generic error arm.
        return Some(opts.host_io_backoff);
    }
    if worker::is_sqlite_disk_full(err) {
        return Some(opts.disk_full_backoff);
    }
    if worker::is_sqlite_io_transient(err) {
        return Some(opts.io_backoff);
    }
    if worker::is_sqlite_busy(err) {
        return Some(opts.busy_backoff);
    }
    Some(opts.error_backoff)
}

/// Sleep for `total`, but wake early (within [`SLEEP_SLICE`]) if [`Shutdown`]
/// is triggered mid-sleep.
async fn interruptible_sleep(total: Duration, shutdown: &Shutdown) {
    let mut remaining = total;
    while !remaining.is_zero() {
        if shutdown.is_triggered() {
            return;
        }
        let slice = remaining.min(SLEEP_SLICE);
        sleep(slice).await;
        remaining -= slice;
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
