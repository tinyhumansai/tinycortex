//! `cover_window` tests: pure planner cases (ported from OpenHuman) plus a
//! couple of integration cases over a seeded store.

use super::*;
use crate::memory::retrieval::test_support::{
    insert_chunks, sample_chunk_at, summary_node, test_config,
};
use crate::memory::tree::SummaryNode;
use chrono::{TimeZone, Utc};

fn summary(id: &str, parent: Option<&str>, level: u32, children: &[&str]) -> SummaryNode {
    summary_node(id, "t1", level, parent, children, id, Utc::now())
}

fn doc_summary(id: &str, doc_id: &str, version_ms: i64, children: &[&str]) -> SummaryNode {
    let mut s = summary(id, Some("merge-root"), 1, children);
    s.doc_id = Some(doc_id.to_string());
    s.version_ms = Some(version_ms);
    s
}

#[test]
fn single_eligible_summary_covers_its_leaves() {
    let eligible = vec![summary("s1", None, 1, &["chunk-a", "chunk-b"])];
    let plan = plan_cover(&eligible, None);
    assert_eq!(plan.maximal_ids, vec!["s1"]);
    assert!(plan.covered_chunk_ids.contains("chunk-a"));
    assert!(plan.covered_chunk_ids.contains("chunk-b"));
    assert_eq!(plan.covered_chunk_ids.len(), 2);
}

#[test]
fn parent_subsumes_child_only_parent_is_maximal() {
    let eligible = vec![
        summary("s2", None, 2, &["s1"]),
        summary("s1", Some("s2"), 1, &["chunk-a", "chunk-b"]),
    ];
    let plan = plan_cover(&eligible, None);
    assert_eq!(plan.maximal_ids, vec!["s2"]);
    assert!(plan.covered_chunk_ids.contains("chunk-a"));
    assert!(plan.covered_chunk_ids.contains("chunk-b"));
}

#[test]
fn ineligible_parent_leaves_child_as_frontier() {
    let eligible = vec![summary("s1", Some("s2-not-eligible"), 1, &["chunk-a"])];
    let plan = plan_cover(&eligible, None);
    assert_eq!(plan.maximal_ids, vec!["s1"]);
    assert!(plan.covered_chunk_ids.contains("chunk-a"));
}

#[test]
fn empty_eligible_set_covers_nothing() {
    let plan = plan_cover(&[], None);
    assert!(plan.maximal_ids.is_empty());
    assert!(plan.covered_chunk_ids.is_empty());
}

#[test]
fn sibling_frontier_nodes_each_emitted() {
    let eligible = vec![
        summary("s1", Some("root-x"), 1, &["chunk-a"]),
        summary("s2", Some("root-x"), 1, &["chunk-b"]),
    ];
    let mut plan = plan_cover(&eligible, None);
    plan.maximal_ids.sort();
    assert_eq!(plan.maximal_ids, vec!["s1", "s2"]);
    assert_eq!(plan.covered_chunk_ids.len(), 2);
}

#[test]
fn filter_superseded_doc_versions_keeps_newest_and_suppresses_old_chunks() {
    let eligible = vec![
        doc_summary("pageA@v1", "notion:pageA", 100, &["chunk-old"]),
        doc_summary("pageA@v2", "notion:pageA", 200, &["chunk-new"]),
        summary("chat", Some("root"), 1, &["chunk-chat"]),
    ];
    let (kept, suppressed) = filter_superseded_doc_versions(eligible);
    let kept_ids: Vec<&str> = kept.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(kept_ids, vec!["pageA@v2", "chat"]);
    assert!(suppressed.contains("chunk-old"));
    assert!(!suppressed.contains("chunk-new"));
    assert!(!suppressed.contains("chunk-chat"));
}

#[test]
fn filter_superseded_doc_versions_dedups_duplicate_winning_revision() {
    let eligible = vec![
        doc_summary("dup-a", "notion:pageB", 300, &["chunk-a"]),
        doc_summary("dup-b", "notion:pageB", 300, &["chunk-b"]),
    ];
    let (kept, suppressed) = filter_superseded_doc_versions(eligible);
    let kept_ids: Vec<&str> = kept.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(kept_ids, vec!["dup-a"]);
    assert!(suppressed.contains("chunk-b"));
    assert!(!suppressed.contains("chunk-a"));
}

#[test]
fn restrict_drops_summaries_spanning_out_of_filter_chunks() {
    let eligible = vec![
        summary("s_mixed", Some("root"), 1, &["chunk-a", "chunk-foreign"]),
        summary("s_clean", Some("root"), 1, &["chunk-b"]),
    ];
    let present: std::collections::HashSet<&str> = ["chunk-a", "chunk-b"].into_iter().collect();
    let plan = plan_cover(&eligible, Some(&present));
    assert_eq!(plan.maximal_ids, vec!["s_clean"]);
    assert!(plan.covered_chunk_ids.contains("chunk-b"));
    assert!(!plan.covered_chunk_ids.contains("chunk-a"));
    assert!(!plan.covered_chunk_ids.contains("chunk-foreign"));
}

#[test]
fn cover_window_until_before_since_errors() {
    let (_tmp, cfg) = test_config();
    let err = cover_window(&cfg, 200, 100, None, None, 0);
    assert!(err.is_err());
}

#[test]
fn cover_window_emits_raw_chunks_when_no_tree() {
    let (_tmp, cfg) = test_config();
    let ts = Utc.timestamp_millis_opt(1_700_000_000_000).unwrap();
    let a = sample_chunk_at("slack:#eng", 0, "a", ts);
    let b = sample_chunk_at("slack:#eng", 1, "b", ts);
    insert_chunks(&cfg, &[a.clone(), b.clone()]);

    let resp = cover_window(
        &cfg,
        ts.timestamp_millis() - 1,
        ts.timestamp_millis() + 1,
        None,
        None,
        0,
    )
    .unwrap();
    assert_eq!(
        resp.hits.len(),
        2,
        "no tree → both in-window chunks emitted raw"
    );
    assert!(resp
        .hits
        .iter()
        .all(|h| h.node_kind == crate::memory::retrieval::NodeKind::Leaf));
}

#[test]
fn scoped_cover_filters_memory_sources_before_result_limit() {
    let (_tmp, cfg) = test_config();
    let ts = Utc.timestamp_millis_opt(1_700_000_000_000).unwrap();
    let mut denied = sample_chunk_at("slack:#denied", 0, "denied", ts);
    denied.metadata.tags.push("memory_sources".to_string());
    let mut allowed = sample_chunk_at("slack:#allowed", 0, "allowed", ts);
    allowed.metadata.tags.push("memory_sources".to_string());
    insert_chunks(&cfg, &[denied, allowed]);

    let scope = Some(["slack:#allowed".to_string()].into_iter().collect());
    let response = cover_window_scoped(
        &cfg,
        ts.timestamp_millis() - 1,
        ts.timestamp_millis() + 1,
        None,
        None,
        scope,
        1,
    )
    .unwrap();

    assert_eq!(response.hits.len(), 1);
    assert_eq!(response.hits[0].tree_scope, "slack:#allowed");
}
