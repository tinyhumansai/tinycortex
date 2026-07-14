//! Deterministic graph-routed retrieval over explicit query entity ids.

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};

use anyhow::Result;

use crate::memory::config::MemoryConfig;
use crate::memory::graph::{pair_distances, PairDistance};
use crate::memory::score::embed::Embedder;
use crate::memory::score::store::lookup_entity;
use crate::memory::tree::store::{get_summaries_batch, get_tree};

use super::fetch::fetch_leaves;
use super::source::query_source;
use super::types::{hydrated_summary_hit, QueryResponse};

const DEFAULT_LIMIT: usize = 10;
const DEFAULT_MAX_HOPS: u32 = 2;

#[derive(Clone, Debug)]
pub struct FastRetrieveOptions {
    pub limit: usize,
    pub max_hops: u32,
    pub time_window_days: Option<u32>,
}

impl Default for FastRetrieveOptions {
    fn default() -> Self {
        Self {
            limit: DEFAULT_LIMIT,
            max_hops: DEFAULT_MAX_HOPS,
            time_window_days: None,
        }
    }
}

pub async fn fast_retrieve(
    config: &MemoryConfig,
    query: &str,
    query_entity_ids: &[String],
    embedder: &dyn Embedder,
    source_scope: Option<&HashSet<String>>,
    options: FastRetrieveOptions,
) -> Result<QueryResponse> {
    let limits = &config.retrieval.limits;
    let limit = if options.limit == 0 {
        limits.default_limit
    } else {
        options.limit.min(limits.max_limit)
    };
    let max_hops = if options.max_hops == 0 {
        limits.default_graph_hops
    } else {
        options.max_hops.min(limits.max_graph_hops)
    };
    let query = query.trim();
    if query.is_empty() {
        return Ok(QueryResponse::empty());
    }
    let entity_ids = dedup_ids(query_entity_ids.iter().cloned());
    log::debug!(
        "[memory_retrieval:fast] entities={} limit={} hops={}",
        entity_ids.len(),
        limit,
        max_hops
    );
    if entity_ids.is_empty() {
        return dense(
            config,
            query,
            embedder,
            source_scope,
            limit,
            options.time_window_days,
        )
        .await;
    }
    let pairs = pair_distances(config, &entity_ids, max_hops)?;
    if pairs.is_empty() {
        return global_occurrence(
            config,
            query,
            &entity_ids,
            embedder,
            source_scope,
            limit,
            options.time_window_days,
        )
        .await;
    }
    let mut hops = max_hops;
    let mut candidates = local_candidates(config, &pairs)?;
    let mut last_non_empty = candidates.clone();
    while candidates.len() > limit && hops > 1 {
        hops -= 1;
        let next = local_candidates(config, &pair_distances(config, &entity_ids, hops)?)?;
        if next.is_empty() {
            break;
        }
        last_non_empty = next.clone();
        candidates = next;
    }
    if candidates.is_empty() {
        candidates = last_non_empty;
    }
    if candidates.is_empty() {
        return global_occurrence(
            config,
            query,
            &entity_ids,
            embedder,
            source_scope,
            limit,
            options.time_window_days,
        )
        .await;
    }
    resolve_local(config, candidates, source_scope, limit)
}

#[derive(Clone, Debug)]
struct Candidate {
    node_kind: String,
    matched: HashSet<String>,
    latest_ts: i64,
}

fn local_candidates(
    config: &MemoryConfig,
    pairs: &[PairDistance],
) -> Result<HashMap<String, Candidate>> {
    let mut output = HashMap::new();
    for pair in pairs {
        let occurrence_limit = config.retrieval.limits.occurrence_lookup_limit;
        let left = lookup_entity(config, &pair.a, Some(occurrence_limit))?;
        let right = lookup_entity(config, &pair.b, Some(occurrence_limit))?;
        let right_nodes: HashMap<_, _> = right
            .iter()
            .map(|hit| (hit.node_id.as_str(), hit.timestamp_ms))
            .collect();
        for hit in left {
            let Some(right_ts) = right_nodes.get(hit.node_id.as_str()) else {
                continue;
            };
            let candidate = output.entry(hit.node_id).or_insert_with(|| Candidate {
                node_kind: hit.node_kind,
                matched: HashSet::new(),
                latest_ts: 0,
            });
            candidate.matched.insert(pair.a.clone());
            candidate.matched.insert(pair.b.clone());
            candidate.latest_ts = candidate.latest_ts.max(hit.timestamp_ms).max(*right_ts);
        }
    }
    Ok(output)
}

fn resolve_local(
    config: &MemoryConfig,
    candidates: HashMap<String, Candidate>,
    source_scope: Option<&HashSet<String>>,
    limit: usize,
) -> Result<QueryResponse> {
    let mut ordered: Vec<_> = candidates.into_iter().collect();
    ordered.sort_by(|a, b| {
        b.1.matched
            .len()
            .cmp(&a.1.matched.len())
            .then_with(|| b.1.latest_ts.cmp(&a.1.latest_ts))
            .then_with(|| a.0.cmp(&b.0))
    });
    let coverage: HashMap<_, _> = ordered
        .iter()
        .map(|(id, candidate)| (id.clone(), candidate.matched.len() as f32))
        .collect();
    let leaf_ids: Vec<_> = ordered
        .iter()
        .filter(|(_, candidate)| candidate.node_kind == "leaf")
        .map(|(id, _)| id.clone())
        .collect();
    let summary_ids: Vec<_> = ordered
        .iter()
        .filter(|(_, candidate)| candidate.node_kind != "leaf")
        .map(|(id, _)| id.clone())
        .collect();
    let mut by_id = HashMap::new();
    for ids in leaf_ids.chunks(config.retrieval.limits.fetch_batch_limit) {
        for hit in fetch_leaves(config, ids)? {
            by_id.insert(hit.node_id.clone(), hit);
        }
    }
    let summaries = get_summaries_batch(config, &summary_ids)?;
    let mut scope_cache: HashMap<String, String> = HashMap::new();
    for (id, node) in summaries {
        let scope = match scope_cache.get(&node.tree_id) {
            Some(scope) => scope.clone(),
            None => {
                let scope = get_tree(config, &node.tree_id)?
                    .map(|tree| tree.scope)
                    .unwrap_or_default();
                scope_cache.insert(node.tree_id.clone(), scope.clone());
                scope
            }
        };
        by_id.insert(id, hydrated_summary_hit(config, &node, &scope));
    }
    let mut hits = Vec::new();
    for (id, _) in ordered {
        if let Some(mut hit) = by_id.remove(&id) {
            if source_scope.is_some_and(|scope| !scope.contains(&hit.tree_scope)) {
                continue;
            }
            hit.score = coverage.get(&id).copied().unwrap_or_default();
            hits.push(hit);
        }
    }
    let total = hits.len();
    hits.truncate(limit);
    Ok(QueryResponse::new(hits, total))
}

async fn dense(
    config: &MemoryConfig,
    query: &str,
    embedder: &dyn Embedder,
    source_scope: Option<&HashSet<String>>,
    limit: usize,
    window: Option<u32>,
) -> Result<QueryResponse> {
    let mut response = query_source(
        config,
        None,
        None,
        window,
        Some(query),
        embedder,
        usize::MAX,
    )
    .await?;
    if let Some(scope) = source_scope {
        response.hits.retain(|hit| scope.contains(&hit.tree_scope));
    }
    let total = response.hits.len();
    response.hits.truncate(limit);
    Ok(QueryResponse::new(response.hits, total))
}

async fn global_occurrence(
    config: &MemoryConfig,
    query: &str,
    entity_ids: &[String],
    embedder: &dyn Embedder,
    source_scope: Option<&HashSet<String>>,
    limit: usize,
    window: Option<u32>,
) -> Result<QueryResponse> {
    let mut response = dense(
        config,
        query,
        embedder,
        source_scope,
        limit.saturating_mul(2),
        window,
    )
    .await?;
    let ids: HashSet<_> = entity_ids.iter().map(String::as_str).collect();
    response.hits.sort_by_key(|hit| {
        Reverse(
            hit.entities
                .iter()
                .filter(|entity| ids.contains(entity.as_str()))
                .count(),
        )
    });
    response.hits.truncate(limit);
    Ok(QueryResponse::new(response.hits, response.total))
}

fn dedup_ids(ids: impl Iterator<Item = String>) -> Vec<String> {
    let mut seen = HashSet::new();
    ids.filter(|id| seen.insert(id.clone())).collect()
}

#[cfg(test)]
#[path = "fast_tests.rs"]
mod tests;
