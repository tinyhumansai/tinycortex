//! `fetch_leaves` — batch-fetch raw chunks by id and project them into the
//! unified [`RetrievalHit`] shape.
//!
//! The contract, ported from OpenHuman's `memory_tree::retrieval::fetch`:
//! "given these chunk ids, give me the full content + metadata so I can cite."
//! The batch is capped at [`MAX_BATCH`] to keep the round-trip bounded. Missing
//! ids are silently skipped — the return is best-effort, so partial failures
//! are visible via `hits.len() < ids.len()`.
//!
//! Each hit is annotated with the chunk's score from `mem_tree_score` when
//! available; score is `0.0` when the chunk has no score row. Two batched
//! reads (chunks + scores) replace `2N` per-id queries.

use anyhow::Result;

use crate::memory::chunks::get_chunks_batch;
use crate::memory::config::MemoryConfig;
use crate::memory::score::store::get_scores_batch;

use super::types::{hydrated_chunk_hit, RetrievalHit};

/// Max batch size. Callers that pass more than this get truncated (no error so
/// the caller still sees a partial result).
pub const MAX_BATCH: usize = 20;

/// Fetch chunk rows by id in the provided order. Missing ids are dropped from
/// the response; input ordering is preserved for the rest.
pub fn fetch_leaves(config: &MemoryConfig, chunk_ids: &[String]) -> Result<Vec<RetrievalHit>> {
    if chunk_ids.is_empty() {
        return Ok(Vec::new());
    }

    let ids: &[String] = if chunk_ids.len() > MAX_BATCH {
        &chunk_ids[..MAX_BATCH]
    } else {
        chunk_ids
    };

    // Two batched SQLite reads up front instead of 2N per-id queries.
    let chunk_by_id = get_chunks_batch(config, ids)?;
    let score_by_id = get_scores_batch(config, ids)?;

    let mut out: Vec<RetrievalHit> = Vec::with_capacity(ids.len());
    for id in ids {
        let Some(chunk) = chunk_by_id.get(id) else {
            continue;
        };
        let score = score_by_id.get(id).copied().unwrap_or(0.0);
        // Leaves are not attached to a materialised tree; `tree_scope` falls
        // back to the chunk's own source id so consumers still see provenance.
        let scope = chunk.metadata.source_id.clone();
        out.push(hydrated_chunk_hit(config, chunk, "", &scope, score));
    }
    Ok(out)
}

#[cfg(test)]
#[path = "fetch_tests.rs"]
mod tests;
