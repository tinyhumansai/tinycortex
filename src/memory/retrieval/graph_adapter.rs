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
//! The adapter implements the graph trait's backend-native co-occurrence fast
//! path as one SQL self-join, without the per-entity occurrence cap. Direct
//! calls to `nodes_for_entity` remain capped for bounded diagnostics.
//!
//! Also recall the crate-level NOTE in [`crate::memory::retrieval`]: nothing
//! in this crate currently calls into `crate::memory::graph` through this
//! adapter outside of tests — it is a correct, unused building block.

use anyhow::Result;

use crate::memory::config::MemoryConfig;
use crate::memory::graph::{EntityOccurrenceIndex, GraphEdge};
use crate::memory::score::store::{list_entity_ids_for_node, lookup_entity};

#[cfg(test)]
pub(crate) const OCCURRENCE_LOOKUP_LIMIT: usize = 500;

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
    fn co_occurring_entities(
        &self,
        subject_entity: &str,
        limit: usize,
    ) -> Result<Option<Vec<GraphEdge>>> {
        crate::memory::chunks::with_connection(self.config, |conn| {
            let limit = limit.min(i64::MAX as usize) as i64;
            let mut stmt = conn.prepare(
                "SELECT b.entity_id, COUNT(DISTINCT a.node_id) AS weight
                   FROM mem_tree_entity_index a
                   JOIN mem_tree_entity_index b ON a.node_id = b.node_id
              LEFT JOIN mem_tree_score score ON score.chunk_id = a.node_id
              LEFT JOIN mem_tree_summaries summary ON summary.id = a.node_id
                  WHERE a.entity_id = ?1 AND b.entity_id <> ?1
                    AND ((a.node_kind = 'summary' AND summary.id IS NOT NULL AND summary.deleted = 0)
                      OR (a.node_kind <> 'summary' AND COALESCE(score.dropped, 0) = 0))
               GROUP BY b.entity_id
               ORDER BY weight DESC, b.entity_id ASC
                  LIMIT ?2",
            )?;
            let rows = stmt
                .query_map(rusqlite::params![subject_entity, limit], |row| {
                    let weight: i64 = row.get(1)?;
                    Ok(GraphEdge {
                        subject: subject_entity.to_string(),
                        object: row.get(0)?,
                        weight: weight.max(0).min(i64::from(u32::MAX)) as u32,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(Some(rows))
        })
    }

    /// Distinct node ids indexed against `entity_id`, newest-first,
    /// deduplicated, capped by retrieval config. Returns `Err` on
    /// a SQLite read failure; an unknown `entity_id` yields `Ok(vec![])`.
    fn nodes_for_entity(&self, entity_id: &str) -> Result<Vec<String>> {
        let hits = lookup_entity(
            self.config,
            entity_id,
            Some(self.config.retrieval.limits.occurrence_lookup_limit),
        )?;
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

    /// Canonical entity ids indexed against `node_id`. This supports the
    /// portable graph fallback and direct diagnostics; production
    /// co-occurrence uses the set-based method above.
    fn entities_on_node(&self, node_id: &str) -> Result<Vec<String>> {
        list_entity_ids_for_node(self.config, node_id)
    }
}
