//! Unit tests for the retrieval wire types. Ported from OpenHuman's
//! `memory_tree::retrieval::types` test module.

use super::*;
use chrono::Utc;

fn sample_hit() -> RetrievalHit {
    RetrievalHit {
        node_id: "sum-1".into(),
        node_kind: NodeKind::Summary,
        tree_id: "tree-1".into(),
        tree_kind: TreeKind::Source,
        tree_scope: "slack:#eng".into(),
        level: 1,
        content: "the sealed summary content".into(),
        entities: vec!["email:alice@example.com".into()],
        topics: vec!["#launch".into()],
        time_range_start: Utc::now(),
        time_range_end: Utc::now(),
        score: 0.75,
        child_ids: vec!["leaf-a".into(), "leaf-b".into()],
        source_ref: None,
    }
}

#[test]
fn node_kind_as_str_round_trips() {
    assert_eq!(NodeKind::Leaf.as_str(), "leaf");
    assert_eq!(NodeKind::Summary.as_str(), "summary");
}

#[test]
fn query_response_truncated_when_total_exceeds_hits() {
    let hit = sample_hit();
    let resp = QueryResponse::new(vec![hit], 5);
    assert_eq!(resp.hits.len(), 1);
    assert_eq!(resp.total, 5);
    assert!(resp.truncated);
}

#[test]
fn query_response_not_truncated_when_all_returned() {
    let hit = sample_hit();
    let resp = QueryResponse::new(vec![hit], 1);
    assert!(!resp.truncated);
}

#[test]
fn query_response_empty_is_inert() {
    let resp = QueryResponse::empty();
    assert!(resp.hits.is_empty());
    assert_eq!(resp.total, 0);
    assert!(!resp.truncated);
}

#[test]
fn retrieval_hit_serde_round_trip() {
    let hit = sample_hit();
    let json = serde_json::to_string(&hit).unwrap();
    let back: RetrievalHit = serde_json::from_str(&json).unwrap();
    assert_eq!(back, hit);
}

#[test]
fn entity_match_serde_round_trip() {
    let m = EntityMatch {
        canonical_id: "email:alice@example.com".into(),
        kind: EntityKind::Email,
        surface: "alice@example.com".into(),
        mention_count: 7,
        last_seen_ms: 1_700_000_000_000,
    };
    let json = serde_json::to_string(&m).unwrap();
    let back: EntityMatch = serde_json::from_str(&json).unwrap();
    assert_eq!(back, m);
}

#[test]
fn leaf_placeholder_is_always_source() {
    assert_eq!(leaf_tree_placeholder(SourceKind::Chat), TreeKind::Source);
    assert_eq!(leaf_tree_placeholder(SourceKind::Email), TreeKind::Source);
    assert_eq!(
        leaf_tree_placeholder(SourceKind::Document),
        TreeKind::Source
    );
}
