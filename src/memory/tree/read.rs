//! Basic tree-walk read.
//!
//! Descends from a start node (or the tree root) through child summaries down to
//! the leaf chunks, bounded by `max_depth`, projecting each visited node into a
//! compact [`TreeReadHit`]. When a `query` is supplied, hits are scored by a
//! cheap lowercase token-overlap heuristic and sorted descending — full
//! embedding-based hybrid retrieval lives in the separate `retrieval` module.

use std::collections::VecDeque;

use anyhow::Result;

use crate::memory::chunks::get_chunks_batch;
use crate::memory::config::MemoryConfig;
use crate::memory::tree::io::{TreeReadHit, TreeReadRequest, TreeReadResult};
use crate::memory::tree::store::{self, SummaryNode};

/// Default cap on returned hits when the request leaves `limit` unset.
const DEFAULT_READ_LIMIT: usize = 50;

/// Walk a tree from `req.start_node_id` (or the root) down to `req.max_depth`
/// levels, returning compact hits. Summary nodes and leaf chunks are both
/// projected. Returns an empty result if the tree or start node is missing.
///
/// # Gotchas (see `TR-13` in `docs/spec/audit/03-tree-archivist-conversations.md`)
/// - The resolved `start` node's `deleted` flag is never checked, nor is its
///   membership in `tree` verified — an explicit `start_node_id` for a
///   tombstoned or foreign summary is still walked and returned.
/// - Only summary children are filtered on `deleted`; the L1 → chunk fan-out
///   below has no deleted-chunk filter, so a soft-deleted chunk can still
///   surface as a hit.
/// - When more than one node exists at a tree's `max_level` (a transient state
///   between sibling seals), `tree.root_id` names only one of them — a root
///   walk silently misses the sibling subtree(s).
pub fn read_tree(config: &MemoryConfig, req: &TreeReadRequest) -> Result<TreeReadResult> {
    let Some(tree) = store::get_tree(config, &req.tree_id)? else {
        return Ok(TreeReadResult {
            hits: Vec::new(),
            total: 0,
            tree_id: req.tree_id.clone(),
        });
    };
    if req.max_depth == 0 {
        return Ok(TreeReadResult::empty(&tree));
    }

    // Resolve the start summary node.
    let start_id = req.start_node_id.clone().or_else(|| tree.root_id.clone());
    let Some(start_id) = start_id else {
        return Ok(TreeReadResult::empty(&tree));
    };
    let Some(start) = store::get_summary(config, &start_id)? else {
        return Ok(TreeReadResult::empty(&tree));
    };

    // BFS over summaries; emit leaf chunks at the bottom (L1 children).
    let mut hits: Vec<TreeReadHit> = Vec::new();
    let mut queue: VecDeque<(SummaryNode, u32)> = VecDeque::new();
    queue.push_back((start, 0));

    while let Some((node, depth)) = queue.pop_front() {
        hits.push(summary_hit(&node));
        if depth + 1 >= req.max_depth {
            continue;
        }
        if node.level >= 2 {
            // Children are summary ids.
            let kids = store::get_summaries_batch(config, &node.child_ids)?;
            for cid in &node.child_ids {
                if let Some(child) = kids.get(cid) {
                    if !child.deleted {
                        queue.push_back((child.clone(), depth + 1));
                    }
                }
            }
        } else {
            // L1 node — children are raw chunk leaves.
            let chunks = get_chunks_batch(config, &node.child_ids)?;
            for cid in &node.child_ids {
                if let Some(c) = chunks.get(cid) {
                    hits.push(TreeReadHit {
                        node_id: c.id.clone(),
                        node_kind: "chunk".to_string(),
                        level: 0,
                        content: c.content.clone(),
                        score: 0.0,
                    });
                }
            }
        }
    }

    // Optional cheap query relevance: token-overlap score, then sort desc.
    if let Some(q) = req.query.as_deref() {
        let terms: Vec<String> = q
            .to_lowercase()
            .split_whitespace()
            .map(|s| s.to_string())
            .collect();
        for h in &mut hits {
            let lc = h.content.to_lowercase();
            let matches = terms.iter().filter(|t| lc.contains(t.as_str())).count();
            h.score = matches as f32;
        }
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }

    let total = hits.len();
    let limit = req.limit.unwrap_or(DEFAULT_READ_LIMIT);
    hits.truncate(limit);

    Ok(TreeReadResult {
        hits,
        total,
        tree_id: tree.id,
    })
}

fn summary_hit(node: &SummaryNode) -> TreeReadHit {
    TreeReadHit {
        node_id: node.id.clone(),
        node_kind: "summary".to_string(),
        level: node.level,
        content: node.content.clone(),
        score: 0.0,
    }
}

#[cfg(test)]
#[path = "read_tests.rs"]
mod tests;
