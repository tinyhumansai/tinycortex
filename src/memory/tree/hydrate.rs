//! Input hydration for seals and raw-leaf fetches.
//!
//! At L0 a buffer holds chunk ids; at L≥1 it holds summary ids. These helpers
//! resolve those ids into [`SummaryInput`]s the summariser folds. Unlike
//! OpenHuman (which staged bodies to disk), TinyCortex reads chunk/summary
//! content inline from SQLite.

use anyhow::Result;

use crate::memory::chunks::{get_chunk, get_chunks_batch, Chunk};
use crate::memory::config::MemoryConfig;
use crate::memory::score::store::{get_score, list_entity_ids_for_node};
use crate::memory::tree::store::get_summaries_batch;
use crate::memory::tree::summarise::SummaryInput;

/// Hydrate inputs for a seal. At level 0 pulls from `mem_tree_chunks` +
/// `mem_tree_score` + the entity index; at ≥1 pulls from `mem_tree_summaries`.
pub(crate) fn hydrate_inputs(
    config: &MemoryConfig,
    level: u32,
    item_ids: &[String],
) -> Result<Vec<SummaryInput>> {
    if level == 0 {
        hydrate_leaf_inputs(config, item_ids)
    } else {
        hydrate_summary_inputs(config, item_ids)
    }
}

fn hydrate_leaf_inputs(config: &MemoryConfig, chunk_ids: &[String]) -> Result<Vec<SummaryInput>> {
    let mut out: Vec<SummaryInput> = Vec::with_capacity(chunk_ids.len());
    for id in chunk_ids {
        let chunk = match get_chunk(config, id)? {
            Some(c) => c,
            None => continue, // missing leaf — skip, mirrors OpenHuman warn+skip
        };
        let score_value = get_score(config, id)?.map(|row| row.total).unwrap_or(0.0);
        // Canonical entity ids come from the inverted index; topics live on the
        // chunk's metadata tags. `UnionFromChildren` rolls these up the tree.
        let entities = list_entity_ids_for_node(config, id).unwrap_or_default();
        out.push(SummaryInput {
            id: chunk.id.clone(),
            content: chunk.content.clone(),
            token_count: chunk.token_count,
            entities,
            topics: chunk.metadata.tags.clone(),
            time_range_start: chunk.metadata.time_range.0,
            time_range_end: chunk.metadata.time_range.1,
            score: score_value,
        });
    }
    Ok(out)
}

pub(crate) fn hydrate_summary_inputs(
    config: &MemoryConfig,
    summary_ids: &[String],
) -> Result<Vec<SummaryInput>> {
    // One batched `SELECT … WHERE id IN (?,…)`. Walk the caller's slice (not the
    // map) so input order is preserved; missing ids are silently skipped.
    let node_by_id = get_summaries_batch(config, summary_ids)?;
    let mut out: Vec<SummaryInput> = Vec::with_capacity(summary_ids.len());
    for id in summary_ids {
        let Some(node) = node_by_id.get(id) else {
            continue;
        };
        let content = crate::memory::store::content::read_summary_body(config, &node.id)
            .unwrap_or_else(|error| {
                log::warn!(
                    "[memory_tree:hydrate] staged summary read failed id={}: {error}; using SQL content",
                    node.id
                );
                node.content.clone()
            });
        out.push(SummaryInput {
            id: node.id.clone(),
            content,
            token_count: node.token_count,
            entities: node.entities.clone(),
            topics: node.topics.clone(),
            time_range_start: node.time_range_start,
            time_range_end: node.time_range_end,
            score: node.score,
        });
    }
    Ok(out)
}

/// Hydrate raw chunk leaves by id, capped at `cap` results. Returns the chunks
/// in the order requested, skipping any that no longer exist. A `cap` of `0`
/// returns an empty vec.
pub fn fetch_leaves(config: &MemoryConfig, chunk_ids: &[String], cap: usize) -> Result<Vec<Chunk>> {
    if cap == 0 || chunk_ids.is_empty() {
        return Ok(Vec::new());
    }
    let by_id = get_chunks_batch(config, chunk_ids)?;
    let mut out = Vec::with_capacity(chunk_ids.len().min(cap));
    for id in chunk_ids {
        if out.len() >= cap {
            break;
        }
        if let Some(c) = by_id.get(id) {
            out.push(c.clone());
        }
    }
    Ok(out)
}
