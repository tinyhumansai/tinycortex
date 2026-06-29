//! Graph type definitions: the derived edge shape and the occurrence-index
//! contract the graph queries read through.

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// A derived co-occurrence edge between two entities.
///
/// Not a triple in the classical sense — there is no explicit predicate. The
/// `weight` field is the count of distinct nodes the pair has both appeared
/// on, which serves as a cheap proxy for relationship strength.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphEdge {
    /// The entity the query was anchored on.
    pub subject: String,
    /// A co-occurring entity sharing at least one node with `subject`.
    pub object: String,
    /// Number of distinct nodes on which `subject` and `object` both appear.
    pub weight: u32,
}

/// Read contract over the entity occurrence index (`mem_tree_entity_index` in
/// OpenHuman) that the co-occurrence graph derives its edges from.
///
/// `memory_graph` deliberately owns no storage. The occurrence index — which
/// records every `(entity_id, node_id)` pair the tree scorer emits — lives in
/// a separate module (`memory_store::entities`). Rather than hard-depending on
/// that module's concrete connection handling, the graph queries take any
/// implementation of this trait by injection. That keeps the derivation pure
/// and trivially testable with an in-memory fixture, while a real SQLite-backed
/// adapter can satisfy the same two reads in production.
///
/// The two operations mirror the only index reads co-occurrence needs and
/// correspond to OpenHuman's SQL SELF-JOIN over `mem_tree_entity_index`:
///
/// - [`nodes_for_entity`] is the `WHERE a.entity_id = ?` side.
/// - [`entities_on_node`] is the `JOIN … ON a.node_id = b.node_id` side.
///
/// Both must return **distinct** ids so that the derived weight equals
/// `COUNT(DISTINCT node_id)`, matching the original query semantics.
///
/// [`nodes_for_entity`]: EntityOccurrenceIndex::nodes_for_entity
/// [`entities_on_node`]: EntityOccurrenceIndex::entities_on_node
pub trait EntityOccurrenceIndex {
    /// Distinct node ids on which `entity_id` has been indexed.
    ///
    /// An entity with no occurrences yields an empty vector (not an error).
    fn nodes_for_entity(&self, entity_id: &str) -> Result<Vec<String>>;

    /// Distinct canonical entity ids indexed against `node_id`.
    ///
    /// A node with no entities yields an empty vector (not an error).
    fn entities_on_node(&self, node_id: &str) -> Result<Vec<String>>;
}

#[cfg(test)]
#[path = "types_tests.rs"]
mod tests;
