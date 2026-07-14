//! Read-only score and entity-index diagnostics.

use anyhow::Result;
use rusqlite::params;

use super::extract::EntityKind;
use super::store::EntityHit;
use crate::memory::chunks::with_connection;
use crate::memory::config::MemoryConfig;

pub fn lookup_entity(
    config: &MemoryConfig,
    entity_id: &str,
    limit: Option<usize>,
) -> Result<Vec<EntityHit>> {
    lookup_entity_in_window(config, entity_id, None, None, limit)
}

pub fn lookup_entity_in_window(
    config: &MemoryConfig,
    entity_id: &str,
    since_ms: Option<i64>,
    until_ms: Option<i64>,
    limit: Option<usize>,
) -> Result<Vec<EntityHit>> {
    let limit = limit.unwrap_or(100).min(i64::MAX as usize) as i64;
    with_connection(config, |conn| {
        let mut stmt = conn.prepare(
            "SELECT entity_id, node_id, node_kind, entity_kind, surface,
                    score, timestamp_ms, tree_id, is_user
             FROM mem_tree_entity_index
             WHERE entity_id = ?1
               AND (?2 IS NULL OR timestamp_ms >= ?2)
               AND (?3 IS NULL OR timestamp_ms <= ?3)
             ORDER BY timestamp_ms DESC LIMIT ?4",
        )?;
        let rows = stmt
            .query_map(params![entity_id, since_ms, until_ms, limit], |row| {
                let raw: String = row.get(3)?;
                let entity_kind = EntityKind::parse(&raw).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        3,
                        rusqlite::types::Type::Text,
                        error.into(),
                    )
                })?;
                Ok(EntityHit {
                    entity_id: row.get(0)?,
                    node_id: row.get(1)?,
                    node_kind: row.get(2)?,
                    entity_kind,
                    surface: row.get(4)?,
                    score: row.get(5)?,
                    timestamp_ms: row.get(6)?,
                    tree_id: row.get(7)?,
                    is_user: row.get::<_, i32>(8)? != 0,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
}

pub fn list_entity_ids_for_node(config: &MemoryConfig, node_id: &str) -> Result<Vec<String>> {
    with_connection(config, |conn| {
        let mut stmt = conn.prepare(
            "SELECT DISTINCT entity_id FROM mem_tree_entity_index
              WHERE node_id = ?1
              ORDER BY score DESC, timestamp_ms DESC, entity_id ASC",
        )?;
        let rows = stmt
            .query_map(params![node_id], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
}

pub fn count_entity_index(config: &MemoryConfig) -> Result<u64> {
    count_table(config, "mem_tree_entity_index")
}

pub fn count_scores(config: &MemoryConfig) -> Result<u64> {
    count_table(config, "mem_tree_score")
}

fn count_table(config: &MemoryConfig, table: &str) -> Result<u64> {
    with_connection(config, |conn| {
        let sql = format!("SELECT COUNT(*) FROM {table}");
        let count: i64 = conn.query_row(&sql, [], |row| row.get(0))?;
        Ok(count.max(0) as u64)
    })
}
