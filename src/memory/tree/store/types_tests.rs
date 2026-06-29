//! Unit tests for the persisted summary-tree types.

use super::*;
use chrono::Utc;

#[test]
fn tree_kind_round_trip() {
    for k in [TreeKind::Source, TreeKind::Topic, TreeKind::Global] {
        assert_eq!(TreeKind::parse(k.as_str()).unwrap(), k);
    }
    assert!(TreeKind::parse("bogus").is_err());
}

#[test]
fn tree_status_round_trip() {
    for s in [TreeStatus::Active, TreeStatus::Archived] {
        assert_eq!(TreeStatus::parse(s.as_str()).unwrap(), s);
    }
    assert!(TreeStatus::parse("live").is_err());
}

#[test]
fn empty_buffer_is_not_stale() {
    let b = Buffer::empty("t1", 0);
    assert!(b.is_empty());
    assert!(!b.is_stale(Utc::now(), chrono::Duration::zero()));
}

#[test]
fn stale_buffer_detected() {
    let past = Utc::now() - chrono::Duration::hours(10);
    let b = Buffer {
        tree_id: "t1".into(),
        level: 0,
        item_ids: vec!["leaf-1".into()],
        token_sum: 100,
        oldest_at: Some(past),
    };
    assert!(b.is_stale(Utc::now(), chrono::Duration::hours(1)));
    assert!(!b.is_stale(Utc::now(), chrono::Duration::hours(20)));
}

#[test]
fn budgets_match_config_defaults() {
    assert_eq!(INPUT_TOKEN_BUDGET, 50_000);
    assert_eq!(OUTPUT_TOKEN_BUDGET, 5_000);
    assert_eq!(SUMMARY_FANOUT, 10);
    assert!(TOPIC_CREATION_THRESHOLD > TOPIC_ARCHIVE_THRESHOLD);
    assert!(TOPIC_RECHECK_EVERY > 0);
}

#[test]
fn fresh_counters_are_zero() {
    let c = HotnessCounters::fresh("email:alice@example.com", 1_700_000_000_000);
    assert_eq!(c.entity_id, "email:alice@example.com");
    assert_eq!(c.mention_count_30d, 0);
    assert_eq!(c.distinct_sources, 0);
    assert_eq!(c.ingests_since_check, 0);
    assert!(c.last_hotness.is_none());
    assert!(c.last_seen_ms.is_none());
    assert_eq!(c.last_updated_ms, 1_700_000_000_000);
}

#[test]
fn stats_projection_mirrors_row() {
    let c = HotnessCounters {
        entity_id: "e".into(),
        mention_count_30d: 5,
        distinct_sources: 2,
        last_seen_ms: Some(42),
        query_hits_30d: 1,
        graph_centrality: Some(0.3),
        ingests_since_check: 4,
        last_hotness: Some(9.9),
        last_updated_ms: 100,
    };
    let s = c.stats();
    assert_eq!(s.mention_count_30d, 5);
    assert_eq!(s.distinct_sources, 2);
    assert_eq!(s.last_seen_ms, Some(42));
    assert_eq!(s.query_hits_30d, 1);
    assert_eq!(s.graph_centrality, Some(0.3));
}
