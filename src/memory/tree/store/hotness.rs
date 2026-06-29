//! Entity-hotness counter persistence (`mem_tree_entity_hotness`).
//!
//! Hotness is a read-only subconscious signal: it gates materialisation of a
//! topic tree for an entity but is itself a side-table, not a tree row.

use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{params, OptionalExtension};

use super::types::HotnessCounters;
use crate::memory::chunks::with_connection;
use crate::memory::config::MemoryConfig;

/// Fetch the hotness row for `entity_id`, or `None` if never seen.
pub fn get(config: &MemoryConfig, entity_id: &str) -> Result<Option<HotnessCounters>> {
    with_connection(config, |conn| {
        let mut stmt = conn.prepare(
            "SELECT entity_id, mention_count_30d, distinct_sources, last_seen_ms,
                    query_hits_30d, graph_centrality, ingests_since_check,
                    last_hotness, last_updated_ms
               FROM mem_tree_entity_hotness WHERE entity_id = ?1",
        )?;
        let row = stmt
            .query_row(params![entity_id], row_to_counters)
            .optional()
            .context("failed to query mem_tree_entity_hotness")?;
        Ok(row)
    })
}

/// Fetch the hotness row, or a fresh (all-zero) row if the entity has never
/// been seen. The fresh row is NOT persisted.
pub fn get_or_fresh(config: &MemoryConfig, entity_id: &str) -> Result<HotnessCounters> {
    match get(config, entity_id)? {
        Some(c) => Ok(c),
        None => Ok(HotnessCounters::fresh(
            entity_id,
            Utc::now().timestamp_millis(),
        )),
    }
}

/// Upsert the full counter row. Idempotent on `entity_id`.
pub fn upsert(config: &MemoryConfig, counters: &HotnessCounters) -> Result<()> {
    with_connection(config, |conn| {
        conn.execute(
            "INSERT INTO mem_tree_entity_hotness (
                entity_id, mention_count_30d, distinct_sources, last_seen_ms,
                query_hits_30d, graph_centrality, ingests_since_check,
                last_hotness, last_updated_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(entity_id) DO UPDATE SET
                mention_count_30d = excluded.mention_count_30d,
                distinct_sources  = excluded.distinct_sources,
                last_seen_ms      = excluded.last_seen_ms,
                query_hits_30d    = excluded.query_hits_30d,
                graph_centrality  = excluded.graph_centrality,
                ingests_since_check = excluded.ingests_since_check,
                last_hotness      = excluded.last_hotness,
                last_updated_ms   = excluded.last_updated_ms",
            params![
                counters.entity_id,
                counters.mention_count_30d,
                counters.distinct_sources,
                counters.last_seen_ms,
                counters.query_hits_30d,
                counters.graph_centrality,
                counters.ingests_since_check,
                counters.last_hotness,
                counters.last_updated_ms,
            ],
        )
        .with_context(|| {
            format!(
                "failed to upsert mem_tree_entity_hotness for {}",
                counters.entity_id
            )
        })?;
        Ok(())
    })
}

/// Count `(node_id) → DISTINCT tree_id` in the entity index for `entity_id`.
pub fn distinct_sources_for(config: &MemoryConfig, entity_id: &str) -> Result<u32> {
    with_connection(config, |conn| {
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(DISTINCT tree_id)
                   FROM mem_tree_entity_index
                  WHERE entity_id = ?1 AND tree_id IS NOT NULL",
                params![entity_id],
                |r| r.get(0),
            )
            .context("failed to count distinct sources")?;
        Ok(n.max(0) as u32)
    })
}

/// Test / diagnostic helper.
pub fn count(config: &MemoryConfig) -> Result<u64> {
    with_connection(config, |conn| {
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM mem_tree_entity_hotness", [], |r| {
                r.get(0)
            })
            .context("failed to count mem_tree_entity_hotness")?;
        Ok(n.max(0) as u64)
    })
}

fn row_to_counters(row: &rusqlite::Row<'_>) -> rusqlite::Result<HotnessCounters> {
    Ok(HotnessCounters {
        entity_id: row.get(0)?,
        mention_count_30d: row.get::<_, i64>(1)?.max(0) as u32,
        distinct_sources: row.get::<_, i64>(2)?.max(0) as u32,
        last_seen_ms: row.get(3)?,
        query_hits_30d: row.get::<_, i64>(4)?.max(0) as u32,
        graph_centrality: row.get(5)?,
        ingests_since_check: row.get::<_, i64>(6)?.max(0) as u32,
        last_hotness: row.get(7)?,
        last_updated_ms: row.get(8)?,
    })
}

#[cfg(test)]
#[path = "hotness_tests.rs"]
mod tests;
