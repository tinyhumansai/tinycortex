//! Test helpers for the jobs runtime — not used in production code paths.

use anyhow::Result;

use crate::memory::config::MemoryConfig;
use crate::memory::queue::handlers::QueueDelegates;

/// Deterministically run queued memory-tree jobs until no immediately
/// claimable work remains. Intended for tests (and synchronous one-shot
/// drains) that need the async pipeline to settle without a background loop.
///
/// A job that `Defer`s reschedules itself into the future, so it is no longer
/// immediately claimable and the drain terminates — exactly as a real worker
/// would leave it parked until its wake-up time.
pub async fn drain_until_idle(config: &MemoryConfig, delegates: &dyn QueueDelegates) -> Result<()> {
    while crate::memory::queue::worker::run_once(config, delegates).await? {}
    Ok(())
}

#[cfg(test)]
#[path = "testing_tests.rs"]
mod tests;
