//! Tests for the co-occurrence graph queries. Ported from OpenHuman
//! `memory_graph::query`, with the SQLite-backed `mem_tree_entity_index`
//! replaced by an injected in-memory [`EntityOccurrenceIndex`] fixture.

use super::*;
use crate::memory::graph::types::EntityOccurrenceIndex;
use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;

/// In-memory stand-in for the occurrence index.
///
/// Stores `(entity_id, node_id)` occurrences the way `index_entity` writes
/// rows into `mem_tree_entity_index`, and answers the two reads the graph
/// derivation needs. `BTreeSet`/`BTreeMap` give distinct ids and a stable,
/// deterministic iteration order — the same guarantees the SQL `DISTINCT`
/// clauses provide.
#[derive(Default)]
struct InMemoryIndex {
    /// entity_id -> set of node ids it occurs on.
    by_entity: BTreeMap<String, BTreeSet<String>>,
    /// node_id -> set of entity ids on it.
    by_node: BTreeMap<String, BTreeSet<String>>,
}

impl InMemoryIndex {
    fn new() -> Self {
        Self::default()
    }

    /// Record one entity occurrence on a node. Idempotent on the
    /// `(entity_id, node_id)` pair, mirroring the composite primary key of
    /// `mem_tree_entity_index`.
    fn index_entity(&mut self, entity_id: &str, node_id: &str) {
        self.by_entity
            .entry(entity_id.to_string())
            .or_default()
            .insert(node_id.to_string());
        self.by_node
            .entry(node_id.to_string())
            .or_default()
            .insert(entity_id.to_string());
    }
}

impl EntityOccurrenceIndex for InMemoryIndex {
    fn nodes_for_entity(&self, entity_id: &str) -> Result<Vec<String>> {
        Ok(self
            .by_entity
            .get(entity_id)
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default())
    }

    fn entities_on_node(&self, node_id: &str) -> Result<Vec<String>> {
        Ok(self
            .by_node
            .get(node_id)
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default())
    }
}

#[test]
fn empty_when_no_co_occurrence() {
    let mut index = InMemoryIndex::new();
    index.index_entity("email:alice@example.com", "leaf-1");
    let edges = co_occurring_entities(&index, "email:alice@example.com", None).unwrap();
    assert!(edges.is_empty());
}

#[test]
fn single_co_occurrence_weight_one() {
    let mut index = InMemoryIndex::new();
    index.index_entity("email:alice@example.com", "leaf-1");
    index.index_entity("email:bob@example.com", "leaf-1");
    let edges = co_occurring_entities(&index, "email:alice@example.com", None).unwrap();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].object, "email:bob@example.com");
    assert_eq!(edges[0].weight, 1);
}

#[test]
fn weight_counts_distinct_nodes_not_rows() {
    let mut index = InMemoryIndex::new();
    // Both on leaf-1, leaf-2, leaf-3 -> weight 3.
    for leaf in &["leaf-1", "leaf-2", "leaf-3"] {
        index.index_entity("email:alice@example.com", leaf);
        index.index_entity("email:bob@example.com", leaf);
    }
    // Re-indexing the same pair must not inflate the weight (distinct nodes).
    index.index_entity("email:alice@example.com", "leaf-1");
    index.index_entity("email:bob@example.com", "leaf-1");
    let edges = co_occurring_entities(&index, "email:alice@example.com", None).unwrap();
    assert_eq!(edges[0].weight, 3);
}

#[test]
fn excludes_self_edges() {
    let mut index = InMemoryIndex::new();
    index.index_entity("email:alice@example.com", "leaf-1");
    index.index_entity("email:alice@example.com", "leaf-2");
    let edges = co_occurring_entities(&index, "email:alice@example.com", None).unwrap();
    assert!(edges.is_empty());
}

#[test]
fn neighbors_returns_ids_in_weight_order() {
    let mut index = InMemoryIndex::new();
    // alice + bob: 2 shared nodes. alice + carol: 1 shared node.
    index.index_entity("email:alice@example.com", "leaf-1");
    index.index_entity("email:bob@example.com", "leaf-1");
    index.index_entity("email:alice@example.com", "leaf-2");
    index.index_entity("email:bob@example.com", "leaf-2");
    index.index_entity("email:alice@example.com", "leaf-3");
    index.index_entity("email:carol@example.com", "leaf-3");
    let ids = neighbors(&index, "email:alice@example.com", None).unwrap();
    assert_eq!(
        ids,
        vec![
            "email:bob@example.com".to_string(),
            "email:carol@example.com".to_string(),
        ]
    );
}

#[test]
fn ties_break_on_object_id_ascending() {
    let mut index = InMemoryIndex::new();
    // bob and carol each share exactly one node with alice -> equal weight.
    index.index_entity("email:alice@example.com", "leaf-1");
    index.index_entity("email:carol@example.com", "leaf-1");
    index.index_entity("email:alice@example.com", "leaf-2");
    index.index_entity("email:bob@example.com", "leaf-2");
    let ids = neighbors(&index, "email:alice@example.com", None).unwrap();
    assert_eq!(
        ids,
        vec![
            "email:bob@example.com".to_string(),
            "email:carol@example.com".to_string(),
        ]
    );
}

#[test]
fn limit_caps_result_set() {
    let mut index = InMemoryIndex::new();
    for i in 0..5 {
        let node = format!("leaf-{i}");
        index.index_entity("email:alice@example.com", &node);
        index.index_entity(&format!("email:n{i}@example.com"), &node);
    }
    let edges = co_occurring_entities(&index, "email:alice@example.com", Some(2)).unwrap();
    assert_eq!(edges.len(), 2);
}

#[test]
fn group_by_weight_buckets_neighbors() {
    let mut index = InMemoryIndex::new();
    // bob: weight 2, carol: weight 1.
    index.index_entity("email:alice@example.com", "leaf-1");
    index.index_entity("email:bob@example.com", "leaf-1");
    index.index_entity("email:alice@example.com", "leaf-2");
    index.index_entity("email:bob@example.com", "leaf-2");
    index.index_entity("email:alice@example.com", "leaf-3");
    index.index_entity("email:carol@example.com", "leaf-3");
    let edges = co_occurring_entities(&index, "email:alice@example.com", None).unwrap();
    let grouped = group_by_weight(edges);
    assert_eq!(
        grouped.get(&2).unwrap(),
        &vec!["email:bob@example.com".to_string()]
    );
    assert_eq!(
        grouped.get(&1).unwrap(),
        &vec!["email:carol@example.com".to_string()]
    );
}
