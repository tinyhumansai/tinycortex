//! Configuration and control types for the background worker loops.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Cooperative shutdown flag shared with running loops.
///
/// Cheap to [`clone`](Clone) (shares one atomic). A host holds one handle,
/// hands clones to [`run_worker`](super::run_worker) /
/// [`run_scheduler`](super::run_scheduler) / [`run`](super::run), and calls
/// [`trigger`](Shutdown::trigger) to ask every loop to stop at its next
/// checkpoint. The loops observe the flag between jobs and during backoff sleeps
/// so shutdown is prompt without aborting an in-flight job mid-handler.
#[derive(Clone, Default)]
pub struct Shutdown(Arc<AtomicBool>);

impl Shutdown {
    /// A fresh, un-triggered shutdown handle.
    pub fn new() -> Self {
        Self::default()
    }

    /// Request that all loops sharing this handle stop.
    pub fn trigger(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    /// Whether shutdown has been requested.
    pub fn is_triggered(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Backoff tuning for the worker loop.
///
/// Mirrors OpenHuman's "back off, don't page" policy: transient SQLite
/// conditions are re-polled after a kind-specific delay, and a persistently
/// full disk backs off long. Corruption is fatal (the loop returns an error so
/// the host can quarantine + rebuild) and has no backoff knob.
#[derive(Clone, Debug)]
pub struct WorkerLoopConfig {
    /// Pause after a poll that found no claimable work.
    pub idle_backoff: Duration,
    /// Pause after transient write-lock contention (`SQLITE_BUSY`/`LOCKED`).
    pub busy_backoff: Duration,
    /// Pause after a transient I/O condition (WAL/shm cold-start, `CANTOPEN`).
    pub io_backoff: Duration,
    /// Pause after `SQLITE_FULL` (disk full): a persistent host condition.
    pub disk_full_backoff: Duration,
    /// Pause after any other error before retrying.
    pub error_backoff: Duration,
}

impl Default for WorkerLoopConfig {
    fn default() -> Self {
        Self {
            idle_backoff: Duration::from_millis(250),
            busy_backoff: Duration::from_millis(100),
            io_backoff: Duration::from_secs(1),
            disk_full_backoff: Duration::from_secs(30),
            error_backoff: Duration::from_millis(500),
        }
    }
}

/// Cadence tuning for the scheduler loop.
///
/// Each tick enqueues a `flush_stale` job (dedupe-suppressed per 3-hour UTC
/// block, so calling it more often than every 3 hours is harmless) and requeues
/// transiently-failed jobs via `self_heal`.
#[derive(Clone, Debug)]
pub struct SchedulerLoopConfig {
    /// Delay between scheduler ticks.
    pub tick: Duration,
}

impl Default for SchedulerLoopConfig {
    fn default() -> Self {
        Self {
            tick: Duration::from_secs(600),
        }
    }
}
