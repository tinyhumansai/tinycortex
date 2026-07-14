//! `drill_down` — walk `child_ids` from a summary node.
//!
//! Primary use case (ported from OpenHuman's `memory_tree::retrieval::drill_down`):
//! the caller gets a summary hit back from `query_source` / `query_topic` and
//! wants the next level down — more summaries (for L2+ nodes) or raw chunks
//! (for L1 nodes). This is a one-step expansion by default; multi-step walks
//! pass `max_depth > 1`.
//!
//! When `query` is `Some`, visited children are reranked by cosine similarity
//! against the query embedding (un-embedded children sort last); when `None`,
//! children are returned in BFS order.
//!
//! Behaviour:
//! - Unknown `node_id` → empty (not an error — the caller can recover).
//! - `max_depth == 0` → empty (a documented no-op).
//! - Leaves have no children; drilling into a leaf id returns empty.
//! - `limit` is optional; when set it truncates the final (reranked) output.
//!
//! NOTE: for versioned document sources, `walk_with_embeddings` has a
//! doc-version / soft-delete ordering bug. The per-document "latest wins"
//! check (`emitted_docs.insert`) runs *before* the soft-delete check
//! (`summary.deleted`). If the newest revision of a document is soft-deleted,
//! it still claims `emitted_docs` for that `doc_id` before being skipped —
//! which permanently suppresses every older (non-deleted) revision at that
//! level, so the document disappears from the walk entirely instead of
//! falling back to the latest surviving revision.

use std::collections::{HashMap, HashSet};

use anyhow::Result;

use crate::memory::chunks::{get_chunk, get_chunk_embeddings_batch, get_chunks_batch};
use crate::memory::config::MemoryConfig;
use crate::memory::score::embed::Embedder;
use crate::memory::tree::store::{get_summaries_batch, get_summary, get_tree, get_trees_batch};

use super::rerank::rerank_by_semantic_similarity;
use super::types::{hydrated_chunk_hit, hydrated_summary_hit, RetrievalHit};

/// Pre-size hint for the next-level BFS frontier.
const EXPECTED_CHILD_FANOUT: usize = 10;

/// A BFS walk's output: hits in BFS order, paired with each hit's stored
/// embedding (`None` when absent) for the optional rerank pass.
type WalkOutput = (Vec<RetrievalHit>, Vec<Option<Vec<f32>>>);

/// Walk the summary hierarchy down `max_depth` levels and return hydrated child
/// hits. Children at level 1 are raw chunks; deeper children are summaries.
///
/// # Errors
///
/// Propagates any underlying store read failure (summary/tree/chunk lookups).
/// An unknown `node_id`, `max_depth == 0`, or drilling into a leaf id are NOT
/// errors — they return `Ok(vec![])`.
pub async fn drill_down(
    config: &MemoryConfig,
    node_id: &str,
    max_depth: u32,
    query: Option<&str>,
    embedder: &dyn Embedder,
    limit: Option<usize>,
) -> Result<Vec<RetrievalHit>> {
    if max_depth == 0 {
        return Ok(Vec::new());
    }

    let (hits, embeddings) = walk_with_embeddings(config, node_id, max_depth)?;

    let hits = match query {
        Some(q) => rerank_by_semantic_similarity(embedder, q, hits, embeddings).await,
        None => hits,
    };

    let hits = match limit {
        Some(n) if hits.len() > n => hits.into_iter().take(n).collect(),
        _ => hits,
    };
    Ok(hits)
}

/// BFS-style expansion up to `max_depth` levels. Returns each hit paired with
/// its stored embedding (if any) so the async rerank pass needs no second DB
/// round-trip. Batched per BFS depth: at most four reads (summaries / trees /
/// chunks / chunk-embeddings) per level.
fn walk_with_embeddings(
    config: &MemoryConfig,
    start_id: &str,
    max_depth: u32,
) -> Result<WalkOutput> {
    let root_summary = get_summary(config, start_id)?;
    let root_tree_scope = match root_summary.as_ref().map(|s| s.tree_id.clone()) {
        Some(tid) => get_tree(config, &tid)?.map(|t| t.scope).unwrap_or_default(),
        None => String::new(),
    };

    let mut out: Vec<RetrievalHit> = Vec::new();
    let mut embeddings: Vec<Option<Vec<f32>>> = Vec::new();

    let start_children: Vec<String> = match root_summary {
        Some(s) => s.child_ids,
        None => {
            // A leaf has no children; an unknown id yields nothing either way.
            let _ = get_chunk(config, start_id)?;
            return Ok((out, embeddings));
        }
    };

    let mut current_level: Vec<String> = start_children;
    let mut depth: u32 = 1;

    // Latest-version-per-document filter (document source trees). Editing a
    // page seals a NEW doc-root at a higher `version_ms` beside the old one; we
    // surface only the newest revision and skip the stale subtree.
    let mut max_version_by_doc: HashMap<String, i64> = HashMap::new();
    let mut emitted_docs: HashSet<String> = HashSet::new();

    while !current_level.is_empty() && depth <= max_depth {
        let mut summary_by_id = get_summaries_batch(config, &current_level)?;

        // Update per-document latest version with any doc-roots on THIS level
        // before walking, so two side-by-side revisions resolve to the newer.
        for id in &current_level {
            if let Some(s) = summary_by_id.get(id).filter(|summary| !summary.deleted) {
                if let Some(doc_id) = s.doc_id.as_deref() {
                    let v = s.version_ms.unwrap_or(i64::MIN);
                    max_version_by_doc
                        .entry(doc_id.to_string())
                        .and_modify(|cur| {
                            if v > *cur {
                                *cur = v;
                            }
                        })
                        .or_insert(v);
                }
            }
        }

        // Distinct tree_ids referenced by this level's summaries.
        let distinct_tree_ids: Vec<String> = {
            let mut seen: HashSet<&str> = HashSet::new();
            let mut ids: Vec<String> = Vec::new();
            for id in &current_level {
                if let Some(s) = summary_by_id.get(id) {
                    if seen.insert(s.tree_id.as_str()) {
                        ids.push(s.tree_id.clone());
                    }
                }
            }
            ids
        };
        let tree_by_id = get_trees_batch(config, &distinct_tree_ids)?;

        // Ids on this level that aren't summaries are candidate chunk leaves.
        let chunk_ids: Vec<String> = current_level
            .iter()
            .filter(|id| !summary_by_id.contains_key(*id))
            .cloned()
            .collect();
        let mut chunk_by_id = get_chunks_batch(config, &chunk_ids)?;
        let emb_by_id = get_chunk_embeddings_batch(config, &chunk_ids)?;

        let mut next_level: Vec<String> = if depth < max_depth {
            Vec::with_capacity(current_level.len() * EXPECTED_CHILD_FANOUT)
        } else {
            Vec::new()
        };

        for id in &current_level {
            if let Some(summary) = summary_by_id.remove(id) {
                if summary.deleted {
                    continue;
                }
                // Latest-wins: skip a doc-root superseded by a newer revision.
                if let Some(doc_id) = summary.doc_id.as_deref() {
                    let v = summary.version_ms.unwrap_or(i64::MIN);
                    if max_version_by_doc.get(doc_id).is_some_and(|&max| v < max) {
                        continue;
                    }
                    // Dedup duplicates at the winning surviving version.
                    if !emitted_docs.insert(doc_id.to_string()) {
                        continue;
                    }
                }
                let scope = tree_by_id
                    .get(&summary.tree_id)
                    .map(|t| t.scope.clone())
                    .unwrap_or_else(|| root_tree_scope.clone());
                embeddings.push(summary.embedding.clone());
                let child_ids = summary.child_ids.clone();
                out.push(hydrated_summary_hit(config, &summary, &scope));
                if depth < max_depth {
                    next_level.extend(child_ids);
                }
                continue;
            }
            if let Some(chunk) = chunk_by_id.remove(id) {
                let emb = emb_by_id.get(id).cloned();
                embeddings.push(emb);
                out.push(hydrated_chunk_hit(
                    config,
                    &chunk,
                    "",
                    &chunk.metadata.source_id,
                    0.0,
                ));
                continue;
            }
            // Child points at nothing (missing row) — skip it.
        }

        current_level = next_level;
        depth += 1;
    }
    Ok((out, embeddings))
}

#[cfg(test)]
#[path = "drill_down_tests.rs"]
mod tests;
