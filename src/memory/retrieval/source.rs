//! `query_source` — retrieve summary hits from per-source trees.
//!
//! Three selection modes, in priority order (ported from OpenHuman's
//! `memory_tree::retrieval::source`):
//! 1. `source_id` Some → one tree lookup via `(kind = source, scope = source_id)`.
//! 2. `source_kind` Some → every source tree whose scope prefix matches the
//!    kind (chat / email / document).
//! 3. Neither → every source tree.
//!
//! For each tree we pull level ≥ 1 summaries. With `time_window_days`, an SQL
//! predicate keeps only summaries whose `[time_range_start, time_range_end]`
//! overlaps `[now − window, now]` before embeddings are hydrated. When `query`
//! is `Some`, hits are reranked by cosine
//! similarity between the query embedding and each summary's stored embedding;
//! otherwise they are ordered newest-first by `time_range_end`.
//!
//! This is a thin read-only view over `mem_tree_trees` and `mem_tree_summaries`
//! — no new indexes or tables.
//!
//! Unwindowed queries still materialize all selected summaries; callers should
//! supply a window for bounded historical retrieval.

use anyhow::{Context, Result};
use chrono::{Duration, Utc};

use crate::memory::chunks::SourceKind;
use crate::memory::config::MemoryConfig;
use crate::memory::score::embed::Embedder;
use crate::memory::tree::store::{
    get_summary_embeddings_batch, get_tree_by_scope, list_summaries_at_level,
    list_summaries_overlapping_window, list_trees_by_kind,
};
use crate::memory::tree::{Tree, TreeKind};

use super::rerank::rerank_by_semantic_similarity;
use super::types::{hydrated_summary_hit, QueryResponse, RetrievalHit};

/// A summary hit paired with its (possibly sidecar-hydrated) embedding, kept
/// together so window-filtering and reranking stay aligned.
pub(crate) type ScoredHit = (RetrievalHit, Option<Vec<f32>>);

/// Retrieve summary hits from the selected source trees.
///
/// `limit` defaults to 10 when 0 and is capped to the configured public limit.
/// The internal `usize::MAX` sentinel bypasses that cap so callers that apply a
/// narrower scope afterward can collect the complete candidate set first.
/// When `query` is `Some`, the (inert-in-tests) `embedder` is used to
/// semantically rerank; otherwise ordering is newest-first. See the
/// module-level NOTE for the cost profile of `time_window_days`.
pub async fn query_source(
    config: &MemoryConfig,
    source_id: Option<&str>,
    source_kind: Option<SourceKind>,
    time_window_days: Option<u32>,
    query: Option<&str>,
    embedder: &dyn Embedder,
    limit: usize,
) -> Result<QueryResponse> {
    let limits = &config.retrieval.limits;
    let limit = if limit == usize::MAX {
        usize::MAX
    } else if limit == 0 {
        limits.default_limit
    } else {
        limit.min(limits.max_limit)
    };

    let window = time_window_days
        .map(|days| -> Result<_> {
            let now = Utc::now();
            let start = now
                .checked_sub_signed(Duration::days(days as i64))
                .context("retrieval time window exceeds supported timestamp range")?;
            Ok((start.timestamp_millis(), now.timestamp_millis()))
        })
        .transpose()?;
    let scored = collect_source_hits(config, source_id, source_kind, window)?;
    let total = scored.len();

    let sorted = order_hits(scored, query, embedder).await;
    let mut sorted = sorted;
    sorted.truncate(limit);
    Ok(QueryResponse::new(sorted, total))
}

/// Order `scored` hits by semantic similarity (when `query` is `Some`) or by
/// recency (`time_range_end` DESC). Shared by `query_source` and `query_global`.
pub(crate) async fn order_hits(
    scored: Vec<ScoredHit>,
    query: Option<&str>,
    embedder: &dyn Embedder,
) -> Vec<RetrievalHit> {
    match query {
        Some(q) => {
            let (hits, embeddings): (Vec<_>, Vec<_>) = scored.into_iter().unzip();
            rerank_by_semantic_similarity(embedder, q, hits, embeddings).await
        }
        None => {
            let mut hits: Vec<RetrievalHit> = scored.into_iter().map(|(h, _)| h).collect();
            hits.sort_by_key(|h| std::cmp::Reverse(h.time_range_end));
            hits
        }
    }
}

/// Walk `mem_tree_trees` + `mem_tree_summaries` and gather every summary under
/// the selected source trees, hydrating each summary's embedding from the
/// per-model sidecar when the legacy in-row column is empty.
///
/// When `window_ms` is present, filtering happens in SQLite before embedding
/// hydration. Without a window this remains O(number of non-deleted summaries
/// across the selected trees), unbounded by any `limit` argument.
pub(crate) fn collect_source_hits(
    config: &MemoryConfig,
    source_id: Option<&str>,
    source_kind: Option<SourceKind>,
    window_ms: Option<(i64, i64)>,
) -> Result<Vec<ScoredHit>> {
    let trees = select_trees(config, source_id, source_kind)?;

    let mut hits: Vec<RetrievalHit> = Vec::new();
    let mut node_ids: Vec<String> = Vec::new();
    let mut embeddings: Vec<Option<Vec<f32>>> = Vec::new();

    for tree in &trees {
        // An un-sealed tree (no levels, no root) has nothing to return.
        if tree.max_level == 0 && tree.root_id.is_none() {
            continue;
        }
        if let Some((since_ms, until_ms)) = window_ms {
            for node in list_summaries_overlapping_window(config, &tree.id, since_ms, until_ms)? {
                if node.deleted {
                    continue;
                }
                node_ids.push(node.id.clone());
                embeddings.push(node.embedding.clone());
                hits.push(hydrated_summary_hit(config, &node, &tree.scope));
            }
        } else {
            for level in 1..=tree.max_level {
                for node in list_summaries_at_level(config, &tree.id, level)? {
                    if node.deleted {
                        continue;
                    }
                    node_ids.push(node.id.clone());
                    embeddings.push(node.embedding.clone());
                    hits.push(hydrated_summary_hit(config, &node, &tree.scope));
                }
            }
        }
    }

    // Hydrate embeddings for summaries whose in-row column is NULL from the
    // per-model sidecar (the post-cutover state). Single batched lookup.
    let unembedded: Vec<String> = node_ids
        .iter()
        .zip(&embeddings)
        .filter(|(_, e)| e.is_none())
        .map(|(id, _)| id.clone())
        .collect();
    if !unembedded.is_empty() {
        let by_id = get_summary_embeddings_batch(config, &unembedded)?;
        for (id, slot) in node_ids.iter().zip(embeddings.iter_mut()) {
            if slot.is_none() {
                if let Some(v) = by_id.get(id) {
                    *slot = Some(v.clone());
                }
            }
        }
    }

    Ok(hits.into_iter().zip(embeddings).collect())
}

/// Resolve the set of source trees to scan. `source_id` has priority, then
/// `source_kind` (via scope prefix matching), then "all source trees".
fn select_trees(
    config: &MemoryConfig,
    source_id: Option<&str>,
    source_kind: Option<SourceKind>,
) -> Result<Vec<Tree>> {
    if let Some(id) = source_id {
        return match get_tree_by_scope(config, TreeKind::Source, id)? {
            Some(t) => Ok(vec![t]),
            None => Ok(Vec::new()),
        };
    }
    let all = list_trees_by_kind(config, TreeKind::Source)?;
    if let Some(kind) = source_kind {
        let prefix = kind.as_str();
        return Ok(all
            .into_iter()
            .filter(|t| scope_matches_kind(&t.scope, prefix))
            .collect());
    }
    Ok(all)
}

/// Platform prefix → canonical source-kind string. Consulted by
/// [`scope_matches_kind`] so a scope like `slack:#eng` classifies as chat.
const PLATFORM_KINDS: &[(&str, &str)] = &[
    // Chat platforms
    ("slack", "chat"),
    ("discord", "chat"),
    ("telegram", "chat"),
    ("whatsapp", "chat"),
    ("irc", "chat"),
    ("matrix", "chat"),
    ("mattermost", "chat"),
    ("lark", "chat"),
    ("signal", "chat"),
    ("imessage", "chat"),
    ("teams", "chat"),
    // Email platforms
    ("gmail", "email"),
    ("imap", "email"),
    ("outlook", "email"),
    ("fastmail", "email"),
    ("protonmail", "email"),
    // Document platforms
    ("notion", "document"),
    ("linear", "document"),
    ("drive", "document"),
    ("googledoc", "document"),
    ("doc", "document"),
    ("dropbox", "document"),
    ("confluence", "document"),
];

/// Decide whether a tree's `scope` falls under `kind_prefix`. Scope is the
/// chunk's `source_id` verbatim (e.g. `slack:#eng`, `gmail:abc`). We check the
/// literal `<kind>:` prefix and the [`PLATFORM_KINDS`] registry. Heuristic by
/// design — callers needing exact matching pass `source_id` directly.
fn scope_matches_kind(scope: &str, kind_prefix: &str) -> bool {
    let lower = scope.to_lowercase();
    if lower.starts_with(&format!("{kind_prefix}:")) {
        return true;
    }
    PLATFORM_KINDS
        .iter()
        .any(|(platform, kind)| *kind == kind_prefix && lower.starts_with(&format!("{platform}:")))
}

#[cfg(test)]
#[path = "source_tests.rs"]
mod tests;
