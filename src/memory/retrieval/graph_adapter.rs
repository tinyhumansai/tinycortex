//! Adapter wiring the persisted entity index into the storage-agnostic
//! [`EntityOccurrenceIndex`] contract that [`crate::memory::graph`] reads
//! through.
//!
//! `memory_graph` owns no storage; its co-occurrence derivation takes any
//! implementation of the two index reads by injection. In production those
//! reads are the score-store fns over `mem_tree_entity_index`. This adapter
//! binds a [`MemoryConfig`] so retrieval can compute graph relevance for an
//! entity without the graph layer depending on SQLite.
//!
//! NOTE: the co-occurrence derivation in `crate::memory::graph` calls
//! [`ConfigEntityIndex::entities_on_node`] once per node returned by
//! [`ConfigEntityIndex::nodes_for_entity`] — i.e. 1 + up to
//! `OCCURRENCE_LOOKUP_LIMIT` separate SQL queries per derivation, rather
//! than a single self-join. Combined with the newest-first truncation at
//! `OCCURRENCE_LOOKUP_LIMIT`, edge weights for popular entities (more
//! occurrences than the cap) are systematically undercounted and
//! "strongest neighbor" ordering can be wrong. See the improvement plan for
//! the planned SQL self-join fast path.
//!
//! Also recall the crate-level NOTE in [`crate::memory::retrieval`]: nothing
//! in this crate currently calls into `crate::memory::graph` through this
//! adapter outside of tests — it is a correct, unused building block.

use anyhow::Result;

use crate::memory::config::MemoryConfig;
use crate::memory::graph::EntityOccurrenceIndex;
use crate::memory::score::store::{list_entity_ids_for_node, lookup_entity};

/// Per-entity lookup cap for the occurrence reads. Popular entities can touch
/// many nodes; this bounds the fan-out while staying well above any realistic
/// `k`.
const OCCURRENCE_LOOKUP_LIMIT: usize = 500;

/// SQLite-backed [`EntityOccurrenceIndex`] over `mem_tree_entity_index`.
pub struct ConfigEntityIndex<'a> {
    config: &'a MemoryConfig,
}

impl<'a> ConfigEntityIndex<'a> {
    /// Bind the adapter to a config handle.
    pub fn new(config: &'a MemoryConfig) -> Self {
        Self { config }
    }
}

impl EntityOccurrenceIndex for ConfigEntityIndex<'_> {
    /// Distinct node ids indexed against `entity_id`, newest-first,
    /// deduplicated, capped at `OCCURRENCE_LOOKUP_LIMIT`. Returns `Err` on
    /// a SQLite read failure; an unknown `entity_id` yields `Ok(vec![])`.
    fn nodes_for_entity(&self, entity_id: &str) -> Result<Vec<String>> {
        let hits = lookup_entity(self.config, entity_id, Some(OCCURRENCE_LOOKUP_LIMIT))?;
        let mut out: Vec<String> = Vec::with_capacity(hits.len());
        // `lookup_entity` already returns distinct (entity_id, node_id) rows
        // ordered newest-first; collect node ids preserving first-seen order.
        let mut seen = std::collections::HashSet::new();
        for h in hits {
            if seen.insert(h.node_id.clone()) {
                out.push(h.node_id);
            }
        }
        Ok(out)
    }

    /// Canonical entity ids indexed against `node_id`. Called once per node
    /// returned by [`nodes_for_entity`](Self::nodes_for_entity) — see the
    /// module-level NOTE on the resulting query-count cost.
    fn entities_on_node(&self, node_id: &str) -> Result<Vec<String>> {
        list_entity_ids_for_node(self.config, node_id)
    }
}
