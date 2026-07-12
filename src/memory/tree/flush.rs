//! Time-based buffer flush for trees.
//!
//! The bucket-seal path only fires when a buffer crosses its token (L0) or
//! sibling-count (L≥1) gate. Low-volume sources can park a buffer below both
//! thresholds indefinitely, hurting recall. [`flush_stale_buffers`] force-seals
//! any **L0** buffer whose `oldest_at` is older than `max_age`. Upper levels are
//! intentionally never force-sealed (that would create degenerate single-child
//! summaries and collapse the tree into a chain); they gate on fan-in naturally.

use anyhow::Result;
use chrono::{DateTime, Duration, Utc};

use crate::memory::config::MemoryConfig;
use crate::memory::tree::bucket_seal::{
    cascade_all_from, cascade_all_from_with_services, LabelStrategy, SealServices,
};
use crate::memory::tree::store::{self, DEFAULT_FLUSH_AGE_SECS};
use crate::memory::tree::summarise::Summariser;

/// Seal every L0 buffer whose oldest item is older than `max_age`. Returns the
/// number of individual seal calls that fired.
pub async fn flush_stale_buffers(
    config: &MemoryConfig,
    max_age: Duration,
    summariser: &dyn Summariser,
    strategy: &LabelStrategy,
) -> Result<usize> {
    flush_stale_buffers_with_services(
        config,
        max_age,
        &SealServices {
            summariser,
            embedder: None,
            observer: &super::bucket_seal::NoopSealObserver,
        },
        strategy,
    )
    .await
}

pub async fn flush_stale_buffers_with_services(
    config: &MemoryConfig,
    max_age: Duration,
    services: &SealServices<'_>,
    strategy: &LabelStrategy,
) -> Result<usize> {
    let now = Utc::now();
    let cutoff = now - max_age;
    let stale = store::list_stale_buffers(config, cutoff)?;

    // One batched fetch over the distinct tree_ids; missing rows are skipped.
    let distinct_tree_ids: Vec<String> = {
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for buf in &stale {
            if seen.insert(buf.tree_id.clone()) {
                out.push(buf.tree_id.clone());
            }
        }
        out
    };
    let tree_by_id = store::get_trees_batch(config, &distinct_tree_ids)?;

    let mut seals = 0;
    for buf in stale {
        let Some(tree) = tree_by_id.get(&buf.tree_id) else {
            continue; // orphan buffer — tree row gone
        };
        let sealed = cascade_all_from_with_services(
            config, tree, buf.level, true, services, strategy, false,
        )
        .await?;
        seals += sealed.len();
    }
    Ok(seals)
}

/// Convenience wrapper using [`DEFAULT_FLUSH_AGE_SECS`].
pub async fn flush_stale_buffers_default(
    config: &MemoryConfig,
    summariser: &dyn Summariser,
    strategy: &LabelStrategy,
) -> Result<usize> {
    flush_stale_buffers(
        config,
        Duration::seconds(DEFAULT_FLUSH_AGE_SECS),
        summariser,
        strategy,
    )
    .await
}

/// Force-seal one tree's L0 buffer now (e.g. "user disconnected this account").
///
/// This always forces the seal, even for an under-budget buffer — that is the
/// whole point of the disconnect path. The `now` parameter is retained for
/// call-site compatibility but is not used to gate the seal; the force is
/// unconditional.
///
/// # Errors
/// Propagates any error from [`cascade_all_from`], including "no tree with id
/// {tree_id}" if the tree row does not exist.
pub async fn force_flush_tree(
    config: &MemoryConfig,
    tree_id: &str,
    now: Option<DateTime<Utc>>,
    summariser: &dyn Summariser,
    strategy: &LabelStrategy,
) -> Result<Vec<String>> {
    let _ = now;
    let tree = store::get_tree(config, tree_id)?
        .ok_or_else(|| anyhow::anyhow!("no tree with id {tree_id}"))?;
    cascade_all_from(config, &tree, 0, true, summariser, strategy).await
}

#[cfg(test)]
#[path = "flush_tests.rs"]
mod tests;
