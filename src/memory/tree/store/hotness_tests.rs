//! Tests for the entity-hotness side-table.

use super::*;
use tempfile::TempDir;

use crate::memory::config::MemoryConfig;
use crate::memory::tree::store::types::HotnessCounters;

fn test_config() -> (TempDir, MemoryConfig) {
    let tmp = TempDir::new().unwrap();
    let cfg = MemoryConfig::new(tmp.path());
    (tmp, cfg)
}

#[test]
fn get_missing_is_none() {
    let (_tmp, cfg) = test_config();
    assert!(get(&cfg, "email:alice@example.com").unwrap().is_none());
}

#[test]
fn get_or_fresh_returns_zero_row() {
    let (_tmp, cfg) = test_config();
    let c = get_or_fresh(&cfg, "email:alice@example.com").unwrap();
    assert_eq!(c.entity_id, "email:alice@example.com");
    assert_eq!(c.mention_count_30d, 0);
    assert_eq!(c.distinct_sources, 0);
    assert!(c.last_hotness.is_none());
    assert_eq!(count(&cfg).unwrap(), 0);
}

#[test]
fn upsert_round_trip() {
    let (_tmp, cfg) = test_config();
    let c = HotnessCounters {
        entity_id: "email:alice@example.com".into(),
        mention_count_30d: 12,
        distinct_sources: 3,
        last_seen_ms: Some(1_700_000_000_000),
        query_hits_30d: 2,
        graph_centrality: Some(0.25),
        ingests_since_check: 40,
        last_hotness: Some(9.5),
        last_updated_ms: 1_700_000_123_000,
    };
    upsert(&cfg, &c).unwrap();
    assert_eq!(get(&cfg, &c.entity_id).unwrap().unwrap(), c);
    assert_eq!(count(&cfg).unwrap(), 1);
}

#[test]
fn upsert_updates_existing_row_and_get_or_fresh_returns_persisted_value() {
    let (_tmp, cfg) = test_config();
    let mut counters = HotnessCounters::fresh("topic:rust", 100);
    upsert(&cfg, &counters).unwrap();
    counters.mention_count_30d = 7;
    counters.query_hits_30d = 3;
    counters.last_hotness = Some(4.5);
    counters.last_updated_ms = 200;
    upsert(&cfg, &counters).unwrap();

    assert_eq!(count(&cfg).unwrap(), 1);
    assert_eq!(get_or_fresh(&cfg, "topic:rust").unwrap(), counters);
}

#[test]
fn distinct_sources_counts_unique_non_null_tree_ids() {
    use crate::memory::chunks::with_connection;

    let (_tmp, cfg) = test_config();
    with_connection(&cfg, |conn| {
        for (node, tree) in [
            ("one", Some("tree-a")),
            ("two", Some("tree-a")),
            ("three", Some("tree-b")),
            ("four", None),
        ] {
            conn.execute(
                "INSERT INTO mem_tree_entity_index
                 (entity_id,node_id,node_kind,entity_kind,surface,score,timestamp_ms,tree_id,is_user)
                 VALUES ('topic:rust',?1,'leaf','topic','rust',1.0,1,?2,0)",
                rusqlite::params![node, tree],
            )?;
        }
        Ok(())
    })
    .unwrap();

    assert_eq!(distinct_sources_for(&cfg, "topic:rust").unwrap(), 2);
    assert_eq!(distinct_sources_for(&cfg, "topic:missing").unwrap(), 0);
}

#[test]
fn negative_legacy_counters_decode_fail_closed_to_zero() {
    use crate::memory::chunks::with_connection;

    let (_tmp, cfg) = test_config();
    with_connection(&cfg, |conn| {
        conn.execute(
            "INSERT INTO mem_tree_entity_hotness
             (entity_id,mention_count_30d,distinct_sources,last_seen_ms,query_hits_30d,
              graph_centrality,ingests_since_check,last_hotness,last_updated_ms)
             VALUES ('legacy',-1,-2,NULL,-3,NULL,-4,NULL,0)",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    let counters = get(&cfg, "legacy").unwrap().unwrap();
    assert_eq!(counters.mention_count_30d, 0);
    assert_eq!(counters.distinct_sources, 0);
    assert_eq!(counters.query_hits_30d, 0);
    assert_eq!(counters.ingests_since_check, 0);
}
