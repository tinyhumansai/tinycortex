//! Memory-queue operations: backfill-progress signalling and the re-embed
//! backfill switch-path trigger.

use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;

use crate::memory::config::MemoryConfig;
use crate::memory::queue::handlers::QueueDelegates;
use crate::memory::queue::store;
use crate::memory::queue::types::{NewJob, ReembedBackfillPayload};

/// Set while a re-embed backfill chain has work pending.
///
/// Read by retrieval layers so an empty vector-search result during the
/// backfill window is interpreted as "not searched yet" rather than "no such
/// memory" — preventing a confidently-wrong "I have no memory of that"
/// mid-re-embed. Set true when a backfill is enqueued / still has rows; cleared
/// when the chain drains. Process-global (resets to `false` on restart; the
/// worker re-sets it on the next backfill tick).
static BACKFILL_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

/// Mark whether a re-embed backfill currently has pending work.
pub fn set_backfill_in_progress(v: bool) {
    BACKFILL_IN_PROGRESS.store(v, Ordering::Relaxed);
}

/// True while a re-embed backfill chain still has rows to process.
pub fn backfill_in_progress() -> bool {
    BACKFILL_IN_PROGRESS.load(Ordering::Relaxed)
}

/// Ensure a re-embed backfill chain exists for the **current** active
/// signature, if (and only if) there is uncovered work.
///
/// This is the switch-path trigger: call it after the embedder config changes
/// (a new signature → every prior row is missing at it). Idempotent: the
/// per-signature dedupe key means at most one chain per space, and a covered
/// space enqueues nothing. The "active signature" and "is there uncovered
/// work?" probes are delegated, since they read the embedding store the queue
/// does not own.
pub fn ensure_reembed_backfill(
    config: &MemoryConfig,
    delegates: &dyn QueueDelegates,
) -> Result<()> {
    if let Some(job) = planned_reembed_backfill(config, delegates)? {
        if store::enqueue(config, &job)?.is_some() {
            set_backfill_in_progress(true);
        }
    }
    Ok(())
}

/// Build the deduplicated re-embed follow-up without persisting it. Workers
/// use this to commit follow-up creation atomically with parent settlement.
pub(crate) fn planned_reembed_backfill(
    config: &MemoryConfig,
    delegates: &dyn QueueDelegates,
) -> Result<Option<NewJob>> {
    let sig = delegates.active_signature(config);
    if delegates.has_uncovered_reembed_work(config, &sig)? {
        let job = NewJob::reembed_backfill(&ReembedBackfillPayload { signature: sig })?;
        return Ok(Some(job));
    }
    Ok(None)
}

#[cfg(test)]
#[path = "ops_tests.rs"]
mod tests;
