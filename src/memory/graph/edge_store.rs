//! Persisted undirected entity co-occurrence edges.

use std::collections::BTreeSet;

use anyhow::Result;
use rusqlite::{params, Transaction};

use crate::memory::chunks::with_connection;
use crate::memory::config::MemoryConfig;

fn order_pair<'a>(a: &'a str, b: &'a str) -> Option<(&'a str, &'a str)> {
    match a.cmp(b) {
        std::cmp::Ordering::Less => Some((a, b)),
        std::cmp::Ordering::Greater => Some((b, a)),
        std::cmp::Ordering::Equal => None,
    }
}

pub fn pairs_from_entities(entity_ids: &[String]) -> Vec<(String, String)> {
    let unique: Vec<_> = entity_ids
        .iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut pairs = Vec::new();
    for left in 0..unique.len() {
        for right in left + 1..unique.len() {
            if let Some((a, b)) = order_pair(unique[left], unique[right]) {
                pairs.push((a.to_string(), b.to_string()));
            }
        }
    }
    pairs
}

pub fn upsert_edges_tx(
    tx: &Transaction<'_>,
    pairs: &[(String, String)],
    timestamp_ms: i64,
) -> Result<usize> {
    let canonical: BTreeSet<_> = pairs
        .iter()
        .filter_map(|(a, b)| order_pair(a, b).map(|(a, b)| (a.to_string(), b.to_string())))
        .collect();
    let mut statement = tx.prepare(
        "INSERT INTO mem_tree_entity_edges (entity_a, entity_b, weight, updated_ms)
         VALUES (?1, ?2, 1, ?3)
         ON CONFLICT(entity_a, entity_b)
         DO UPDATE SET weight = weight + 1, updated_ms = ?3",
    )?;
    for (a, b) in &canonical {
        statement.execute(params![a, b, timestamp_ms])?;
    }
    Ok(canonical.len())
}

pub fn upsert_edges(
    config: &MemoryConfig,
    pairs: &[(String, String)],
    timestamp_ms: i64,
) -> Result<usize> {
    if pairs.is_empty() {
        return Ok(0);
    }
    with_connection(config, |connection| {
        let transaction = connection.unchecked_transaction()?;
        let count = upsert_edges_tx(&transaction, pairs, timestamp_ms)?;
        transaction.commit()?;
        Ok(count)
    })
}

pub fn edge_neighbors(config: &MemoryConfig, entity_id: &str) -> Result<Vec<(String, i64)>> {
    with_connection(config, |connection| {
        let mut statement = connection.prepare(
            "SELECT entity_b, weight FROM mem_tree_entity_edges WHERE entity_a = ?1
             UNION ALL
             SELECT entity_a, weight FROM mem_tree_entity_edges WHERE entity_b = ?1
             ORDER BY weight DESC",
        )?;
        let rows = statement
            .query_map(params![entity_id], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
}

pub fn clear_edges_for_entities_tx(tx: &Transaction<'_>, entity_ids: &[String]) -> Result<usize> {
    let mut removed = 0;
    let mut statement =
        tx.prepare("DELETE FROM mem_tree_entity_edges WHERE entity_a = ?1 OR entity_b = ?1")?;
    for id in entity_ids {
        removed += statement.execute(params![id])?;
    }
    Ok(removed)
}

pub fn count_edges(config: &MemoryConfig) -> Result<u64> {
    with_connection(config, |connection| {
        let count: i64 =
            connection.query_row("SELECT COUNT(*) FROM mem_tree_entity_edges", [], |row| {
                row.get(0)
            })?;
        Ok(count.max(0) as u64)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persisted_edges_are_canonical_weighted_and_symmetric() {
        let temp = tempfile::tempdir().unwrap();
        let config = MemoryConfig::new(temp.path());
        let pairs = pairs_from_entities(&[
            "person:bob".into(),
            "person:alice".into(),
            "person:alice".into(),
        ]);
        assert_eq!(pairs, vec![("person:alice".into(), "person:bob".into())]);
        upsert_edges(&config, &pairs, 1).unwrap();
        upsert_edges(&config, &pairs, 2).unwrap();
        assert_eq!(
            edge_neighbors(&config, "person:bob").unwrap(),
            vec![("person:alice".into(), 2)]
        );
        assert_eq!(count_edges(&config).unwrap(), 1);
    }
}
