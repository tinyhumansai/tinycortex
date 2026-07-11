//! Periodic `flush_stale` enqueue + transient-failure self-heal.
//!
//! OpenHuman ran these on a `tokio` 3-hourly wall-clock loop. `tokio` is a
//! dev-only dependency here and timer-spawn plumbing is out of the ported
//! surface, so the loop body is exposed as plain functions a host calls on its
//! own schedule. [`enqueue_flush_stale`] is dedupe-suppressed per 3-hour UTC
//! block (so a host can call it freely), and [`self_heal`] requeues
//! transiently-failed jobs while leaving unrecoverable ones parked.

use anyhow::Result;
use chrono::{Timelike, Utc};

use crate::memory::config::MemoryConfig;
use crate::memory::queue::store;
use crate::memory::queue::store_settle::requeue_transient_failed;
use crate::memory::queue::types::{FlushStalePayload, NewJob};

/// Enqueue a `flush_stale` job scoped to the current 3-hour UTC block. Returns
/// `Ok(Some(id))` if enqueued, `Ok(None)` if dedupe-suppressed (another flush
/// already queued for this block). A single `Utc::now()` reading derives both
/// the date and the block so the dedupe key can't disagree with itself across a
/// 3-hour boundary.
pub fn enqueue_flush_stale(config: &MemoryConfig) -> Result<Option<String>> {
    let now = Utc::now();
    let today_iso = now.date_naive().format("%Y-%m-%d").to_string();
    let hour_block = now.hour() / 3;
    let new_job = NewJob::flush_stale(&FlushStalePayload::default(), &today_iso, hour_block)?;
    store::enqueue(config, &new_job)
}

/// Requeue jobs that failed for transient reasons (network blips, timeouts,
/// SQLITE_BUSY) so chunks never sit unprocessed until the next manual sync.
/// Unrecoverable failures stay parked. Returns the number requeued.
///
/// This only touches terminally-`failed` rows. It does **not** call
/// [`recover_stale_locks`](crate::memory::queue::store_settle::recover_stale_locks)
/// — a row stranded `running` past its lease (a crashed sibling process
/// sharing `chunks.db`, or a settle write that itself failed) is invisible to
/// both `self_heal` and `claim_next` until something calls
/// `recover_stale_locks` explicitly. In this crate that only happens once, at
/// [`crate::memory::queue::worker::bootstrap`] time — a long-lived process
/// that only drives the scheduler tick never recovers such a row on its own.
pub fn self_heal(config: &MemoryConfig) -> Result<u64> {
    requeue_transient_failed(config)
}

#[cfg(test)]
#[path = "scheduler_tests.rs"]
mod tests;
