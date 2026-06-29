//! Read-only graph queries derived from an entity occurrence index.
//!
//! OpenHuman expressed co-occurrence as a SQL SELF-JOIN over
//! `mem_tree_entity_index`:
//!
//! ```sql
//! SELECT b.entity_id AS object, COUNT(DISTINCT a.node_id) AS weight
//!   FROM mem_tree_entity_index a
//!   JOIN mem_tree_entity_index b ON a.node_id = b.node_id
//!  WHERE a.entity_id = ?1 AND b.entity_id <> ?1
//!  GROUP BY b.entity_id
//!  ORDER BY weight DESC, object ASC
//!  LIMIT ?2
//! ```
//!
//! Here the same derivation runs in Rust over the injected
//! [`EntityOccurrenceIndex`] so the graph stays decoupled from storage: gather
//! the subject's nodes, fan out to the entities sharing each node, and count
//! distinct shared nodes per neighbour.

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result};

use crate::memory::graph::types::{EntityOccurrenceIndex, GraphEdge};

/// Default cap on the number of edges returned when the caller passes `None`.
const DEFAULT_LIMIT: usize = 100;

/// Return every entity that shares at least one node with `subject_entity`,
/// with a `weight` equal to the number of distinct shared nodes.
///
/// Results are sorted by weight DESC, then object id ASC, for deterministic
/// output regardless of the index's iteration order. Self-edges are excluded.
/// `limit` caps the result set; `None` defaults to [`DEFAULT_LIMIT`].
pub fn co_occurring_entities(
    index: &dyn EntityOccurrenceIndex,
    subject_entity: &str,
    limit: Option<usize>,
) -> Result<Vec<GraphEdge>> {
    let cap = limit.unwrap_or(DEFAULT_LIMIT);

    // For each neighbour, the set of distinct nodes shared with the subject.
    // Using a set (rather than a bare counter) makes the distinct-node count
    // robust even if an index implementation returns a node more than once.
    let mut shared: HashMap<String, HashSet<String>> = HashMap::new();

    let subject_nodes = index
        .nodes_for_entity(subject_entity)
        .with_context(|| format!("nodes_for_entity({subject_entity})"))?;

    for node_id in subject_nodes {
        let entities = index
            .entities_on_node(&node_id)
            .with_context(|| format!("entities_on_node({node_id})"))?;
        for object in entities {
            if object == subject_entity {
                continue;
            }
            shared.entry(object).or_default().insert(node_id.clone());
        }
    }

    let mut edges: Vec<GraphEdge> = shared
        .into_iter()
        .map(|(object, nodes)| GraphEdge {
            subject: subject_entity.to_string(),
            object,
            weight: nodes.len().min(u32::MAX as usize) as u32,
        })
        .collect();

    // weight DESC, then object ASC — mirrors the SQL ORDER BY.
    edges.sort_by(|a, b| {
        b.weight
            .cmp(&a.weight)
            .then_with(|| a.object.cmp(&b.object))
    });
    edges.truncate(cap);
    Ok(edges)
}

/// Convenience wrapper around [`co_occurring_entities`] that returns just the
/// neighbour entity ids in weight-descending order.
pub fn neighbors(
    index: &dyn EntityOccurrenceIndex,
    subject_entity: &str,
    limit: Option<usize>,
) -> Result<Vec<String>> {
    Ok(co_occurring_entities(index, subject_entity, limit)?
        .into_iter()
        .map(|e| e.object)
        .collect())
}

/// Group co-occurrence edges by weight. Useful for UIs that want to render
/// strong vs weak relationships separately. Kept as a pure derivation helper
/// rather than living on the type.
pub fn group_by_weight(edges: Vec<GraphEdge>) -> HashMap<u32, Vec<String>> {
    let mut out: HashMap<u32, Vec<String>> = HashMap::new();
    for e in edges {
        out.entry(e.weight).or_default().push(e.object);
    }
    out
}

#[cfg(test)]
#[path = "query_tests.rs"]
mod tests;
