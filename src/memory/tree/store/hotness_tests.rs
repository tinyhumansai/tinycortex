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
