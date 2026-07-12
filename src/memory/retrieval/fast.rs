//! Deterministic graph-routed retrieval over explicit query entity ids.

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};

use anyhow::Result;

use crate::memory::config::MemoryConfig;
use crate::memory::graph::{pair_distances, PairDistance};
use crate::memory::score::embed::Embedder;
use crate::memory::score::store::lookup_entity;
use crate::memory::tree::store::{get_summaries_batch, get_tree};

use super::fetch::{fetch_leaves, MAX_BATCH};
use super::source::query_source;
use super::types::{hydrated_summary_hit, QueryResponse};

const LOOKUP_LIMIT: usize = 500;
const DEFAULT_LIMIT: usize = 10;
const MAX_RETRIEVE_LIMIT: usize = 100;
const DEFAULT_MAX_HOPS: u32 = 2;
const MAX_GRAPH_HOPS: u32 = 4;

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
    let limit = if options.limit == 0 {
        DEFAULT_LIMIT
    } else {
        options.limit.min(MAX_RETRIEVE_LIMIT)
    };
    let max_hops = if options.max_hops == 0 {
        DEFAULT_MAX_HOPS
    } else {
        options.max_hops.min(MAX_GRAPH_HOPS)
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
        let left = lookup_entity(config, &pair.a, Some(LOOKUP_LIMIT))?;
        let right = lookup_entity(config, &pair.b, Some(LOOKUP_LIMIT))?;
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
    for ids in leaf_ids.chunks(MAX_BATCH) {
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
mod tests {
    use super::*;
    use crate::memory::graph::{pairs_from_entities, upsert_edges};
    use crate::memory::retrieval::test_support::{
        fixed_ts, index_entity_occurrence, insert_summary, insert_tree_row, source_tree,
        summary_node, test_config,
    };
    use crate::memory::score::embed::InertEmbedder;
    use crate::memory::score::extract::EntityKind;

    #[test]
    fn options_and_ids_are_bounded_deterministically() {
        assert_eq!(FastRetrieveOptions::default().limit, 10);
        assert_eq!(
            dedup_ids(vec!["a".into(), "a".into(), "b".into()].into_iter()),
            vec!["a", "b"]
        );
    }

    #[tokio::test]
    async fn local_branch_intersects_entities_and_applies_scope_before_limit() {
        let (_temp, config) = test_config();
        let allowed_tree = source_tree("allowed-tree", "slack:#allowed", Some("allowed"), 1);
        let denied_tree = source_tree("denied-tree", "slack:#denied", Some("denied"), 1);
        insert_tree_row(&config, &allowed_tree);
        insert_tree_row(&config, &denied_tree);
        for (id, tree_id) in [("allowed", "allowed-tree"), ("denied", "denied-tree")] {
            insert_summary(
                &config,
                &summary_node(id, tree_id, 1, None, &[], id, fixed_ts()),
            );
            for (entity, kind) in [
                ("person:alice", EntityKind::Person),
                ("topic:launch", EntityKind::Topic),
            ] {
                index_entity_occurrence(
                    &config,
                    entity,
                    kind,
                    entity,
                    id,
                    "summary",
                    fixed_ts().timestamp_millis(),
                    Some(tree_id),
                );
            }
        }
        upsert_edges(
            &config,
            &pairs_from_entities(&["person:alice".into(), "topic:launch".into()]),
            fixed_ts().timestamp_millis(),
        )
        .unwrap();
        let scope = HashSet::from(["slack:#allowed".to_string()]);

        let response = fast_retrieve(
            &config,
            "alice launch",
            &["person:alice".into(), "topic:launch".into()],
            &InertEmbedder,
            Some(&scope),
            FastRetrieveOptions {
                limit: 1,
                ..Default::default()
            },
        )
        .await
        .unwrap();

        assert_eq!(response.total, 1);
        assert_eq!(response.hits[0].node_id, "allowed");
        assert_eq!(response.hits[0].score, 2.0);
    }
}
