//! `query_global` and `query_topic` — the time-axis and subject-axis
//! projections.
//!
//! OpenHuman retired its standalone global (time) and topic (subject) trees:
//! "source trees hold all the content, and walking the source hierarchy plus
//! the entity index reconstructs both projections." These two primitives are
//! that reconstruction:
//!
//! - [`query_global`] is a cross-source digest over an explicit `[since, until]`
//!   window — every source tree's summaries whose envelope overlaps the window,
//!   ordered by recency or by semantic similarity to a query.
//! - [`query_topic`] is entity/topic-scoped: it walks the entity index for a
//!   canonical id and resolves the indexed nodes (summaries and leaves) into
//!   hits, optionally windowed and reranked.

use anyhow::Result;
use chrono::{DateTime, TimeZone, Utc};

use crate::memory::chunks::{get_chunk_embeddings_batch, get_chunks_batch, SourceKind};
use crate::memory::config::MemoryConfig;
use crate::memory::score::embed::Embedder;
use crate::memory::score::store::lookup_entity;
use crate::memory::tree::store::{
    get_summaries_batch, get_summary_embeddings_batch, get_trees_batch,
};

use super::source::{collect_source_hits, order_hits, ScoredHit};
use super::types::{hit_from_chunk, hit_from_summary, QueryResponse};

/// Default result cap applied by both primitives when the caller passes
/// `limit = 0`.
const DEFAULT_LIMIT: usize = 10;

/// Per-entity node cap for the topic projection: `resolve_topic_hits` reads
/// at most this many newest entity-index rows for a given `entity_id` before
/// any window filter is applied (see the gotcha documented on
/// [`query_topic`]).
const TOPIC_LOOKUP_CAP: usize = 200;

/// Cross-source digest over `[since_ms, until_ms]`.
///
/// Gathers summaries from every source tree (optionally narrowed by
/// `source_kind`) whose time envelope overlaps the window, then orders them by
/// recency, or — when `query` is `Some` — by semantic similarity. `limit`
/// defaults to 10 when 0.
///
/// Returns an error if `until_ms < since_ms`.
///
/// Cost note: the window filter is applied in Rust, after
/// `collect_source_hits` has materialized every non-deleted summary across
/// every selected source tree (embeddings included) — see
/// `collect_source_hits`'s docs for why this does not scale with store
/// size.
pub async fn query_global(
    config: &MemoryConfig,
    since_ms: i64,
    until_ms: i64,
    source_kind: Option<SourceKind>,
    query: Option<&str>,
    embedder: &dyn Embedder,
    limit: usize,
) -> Result<QueryResponse> {
    let limit = if limit == 0 { DEFAULT_LIMIT } else { limit };
    if until_ms < since_ms {
        return Err(anyhow::anyhow!(
            "query_global: until_ms ({until_ms}) precedes since_ms ({since_ms})"
        ));
    }
    let since = ms_to_utc(since_ms);
    let until = ms_to_utc(until_ms);

    let mut scored = collect_source_hits(config, None, source_kind)?;
    scored.retain(|(h, _)| h.time_range_end >= since && h.time_range_start <= until);
    let total = scored.len();

    let mut hits = order_hits(scored, query, embedder).await;
    hits.truncate(limit);
    Ok(QueryResponse::new(hits, total))
}

/// Entity/topic-scoped retrieval.
///
/// `entity_id` is a canonical id (e.g. `topic:phoenix`,
/// `email:alice@example.com`). Every node indexed against it is resolved into a
/// hit (summaries hydrated with their tree scope, leaves with their source id),
/// optionally restricted to `[since_ms, until_ms]` and reranked by `query`.
///
/// # Gotcha: the window filter runs after a hard row cap
///
/// `resolve_topic_hits` caps the entity-index lookup at `TOPIC_LOOKUP_CAP`
/// newest rows *before* the `since_ms`/`until_ms` window is applied here. For
/// an entity with more mentions than the cap, a window reaching further back
/// than the cap's newest rows can return zero or partial hits even though
/// matching rows exist further back in the index, and the returned `total`
/// under-reports the true match count for that window. Callers should not
/// assume completeness for a high-mention entity queried with an old window.
pub async fn query_topic(
    config: &MemoryConfig,
    entity_id: &str,
    since_ms: Option<i64>,
    until_ms: Option<i64>,
    query: Option<&str>,
    embedder: &dyn Embedder,
    limit: usize,
) -> Result<QueryResponse> {
    let limit = if limit == 0 { DEFAULT_LIMIT } else { limit };
    let entity_id = entity_id.trim();
    if entity_id.is_empty() {
        return Ok(QueryResponse::empty());
    }

    let mut scored = resolve_topic_hits(config, entity_id)?;

    if since_ms.is_some() || until_ms.is_some() {
        let since = since_ms.map(ms_to_utc);
        let until = until_ms.map(ms_to_utc);
        scored.retain(|(h, _)| {
            since.is_none_or(|s| h.time_range_end >= s)
                && until.is_none_or(|u| h.time_range_start <= u)
        });
    }
    let total = scored.len();

    let mut hits = order_hits(scored, query, embedder).await;
    hits.truncate(limit);
    Ok(QueryResponse::new(hits, total))
}

/// Resolve every node indexed against `entity_id` into a [`ScoredHit`],
/// preserving the entity index's newest-first order and hydrating embeddings
/// (summary sidecar / chunk sidecar) for the optional rerank pass.
///
/// Caps the lookup at `TOPIC_LOOKUP_CAP` rows (see the caller-facing gotcha
/// on [`query_topic`]). Soft-deleted summaries are excluded (`node.deleted`
/// check below); leaf chunks have no equivalent tombstone check here, so a
/// dropped chunk that is still indexed can still surface in topic results.
/// An entity-index row whose `node_id` resolves to neither a summary nor a
/// chunk (a stale index entry) is silently skipped.
fn resolve_topic_hits(config: &MemoryConfig, entity_id: &str) -> Result<Vec<ScoredHit>> {
    let entity_hits = lookup_entity(config, entity_id, Some(TOPIC_LOOKUP_CAP))?;
    if entity_hits.is_empty() {
        return Ok(Vec::new());
    }

    let summary_ids: Vec<String> = entity_hits
        .iter()
        .filter(|h| h.node_kind == "summary")
        .map(|h| h.node_id.clone())
        .collect();
    let leaf_ids: Vec<String> = entity_hits
        .iter()
        .filter(|h| h.node_kind != "summary")
        .map(|h| h.node_id.clone())
        .collect();

    let summaries = get_summaries_batch(config, &summary_ids)?;
    let summary_embs = get_summary_embeddings_batch(config, &summary_ids)?;
    let chunks = get_chunks_batch(config, &leaf_ids)?;
    let chunk_embs = get_chunk_embeddings_batch(config, &leaf_ids)?;

    // Distinct tree ids for summary scopes.
    let tree_ids: Vec<String> = {
        let mut seen = std::collections::HashSet::new();
        summaries
            .values()
            .filter(|s| seen.insert(s.tree_id.clone()))
            .map(|s| s.tree_id.clone())
            .collect()
    };
    let trees = get_trees_batch(config, &tree_ids)?;

    let mut out: Vec<ScoredHit> = Vec::with_capacity(entity_hits.len());
    for eh in &entity_hits {
        if let Some(node) = summaries.get(&eh.node_id) {
            if node.deleted {
                continue;
            }
            let scope = trees
                .get(&node.tree_id)
                .map(|t| t.scope.clone())
                .unwrap_or_default();
            let emb = node
                .embedding
                .clone()
                .or_else(|| summary_embs.get(&node.id).cloned());
            out.push((hit_from_summary(node, &scope), emb));
        } else if let Some(chunk) = chunks.get(&eh.node_id) {
            let emb = chunk_embs.get(&chunk.id).cloned();
            out.push((
                hit_from_chunk(chunk, "", &chunk.metadata.source_id, 0.0),
                emb,
            ));
        }
    }
    Ok(out)
}

/// Epoch-milliseconds → UTC.
///
/// NOTE: despite the name suggesting a saturating conversion, out-of-range
/// input (e.g. `i64::MIN`/`i64::MAX` sentinels, or any `ms` outside
/// `chrono`'s representable range) does NOT saturate to `DateTime::<Utc>::MIN_UTC`
/// / `MAX_UTC` — it falls back to `Utc::now()`. A caller passing a sentinel
/// meant to mean "no lower/upper bound" for `since_ms`/`until_ms` will
/// instead get "now", silently narrowing the intended window.
fn ms_to_utc(ms: i64) -> DateTime<Utc> {
    Utc.timestamp_millis_opt(ms)
        .single()
        .unwrap_or_else(Utc::now)
}

#[cfg(test)]
#[path = "global_tests.rs"]
mod tests;
