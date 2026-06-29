//! `query_source` tests. Trees + summaries are seeded directly via the store
//! (no seal pipeline); embeddings are inert unless a test writes the sidecar.

use super::*;
use crate::memory::retrieval::test_support::{
    fixed_ts, insert_summary, insert_tree_row, set_summary_embedding, source_tree, summary_node,
    test_config,
};
use crate::memory::score::embed::{InertEmbedder, EMBEDDING_DIM};
use chrono::{DateTime, Duration, TimeZone, Utc};

fn tree_id_for(scope: &str) -> String {
    format!("tree:{scope}")
}
fn summary_id_for(scope: &str) -> String {
    format!("s:{scope}:L1")
}

/// Seed one source tree at `scope` with a single sealed L1 summary at `ts`.
fn seed_source(cfg: &crate::memory::config::MemoryConfig, scope: &str, ts: DateTime<Utc>) {
    let tree_id = tree_id_for(scope);
    let sum_id = summary_id_for(scope);
    let tree = source_tree(&tree_id, scope, Some(&sum_id), 1);
    insert_tree_row(cfg, &tree);
    let node = summary_node(
        &sum_id,
        &tree_id,
        1,
        None,
        &["leaf-a", "leaf-b"],
        "summary",
        ts,
    );
    insert_summary(cfg, &node);
}

fn inert() -> InertEmbedder {
    InertEmbedder::new()
}

fn unit_vec(axis: usize) -> Vec<f32> {
    let mut v = vec![0.0_f32; EMBEDDING_DIM];
    v[axis] = 1.0;
    v
}

#[tokio::test]
async fn query_by_source_id_returns_tree_summaries() {
    let (_tmp, cfg) = test_config();
    seed_source(&cfg, "slack:#eng", fixed_ts());
    let resp = query_source(&cfg, Some("slack:#eng"), None, None, None, &inert(), 10)
        .await
        .unwrap();
    assert_eq!(resp.hits.len(), 1);
    assert_eq!(resp.total, 1);
    assert!(!resp.truncated);
    assert_eq!(resp.hits[0].tree_scope, "slack:#eng");
    assert_eq!(resp.hits[0].level, 1);
}

#[tokio::test]
async fn query_unknown_source_id_returns_empty() {
    let (_tmp, cfg) = test_config();
    let resp = query_source(&cfg, Some("slack:#nope"), None, None, None, &inert(), 10)
        .await
        .unwrap();
    assert!(resp.hits.is_empty());
    assert_eq!(resp.total, 0);
}

#[tokio::test]
async fn query_by_source_kind_filters_scopes() {
    let (_tmp, cfg) = test_config();
    seed_source(&cfg, "slack:#eng", fixed_ts());
    seed_source(&cfg, "gmail:alice@example.com", fixed_ts());

    let chat_only = query_source(&cfg, None, Some(SourceKind::Chat), None, None, &inert(), 10)
        .await
        .unwrap();
    assert_eq!(chat_only.hits.len(), 1);
    assert_eq!(chat_only.hits[0].tree_scope, "slack:#eng");

    let email_only = query_source(
        &cfg,
        None,
        Some(SourceKind::Email),
        None,
        None,
        &inert(),
        10,
    )
    .await
    .unwrap();
    assert_eq!(email_only.hits.len(), 1);
    assert_eq!(email_only.hits[0].tree_scope, "gmail:alice@example.com");
}

#[tokio::test]
async fn query_all_source_trees_when_no_filter() {
    let (_tmp, cfg) = test_config();
    seed_source(&cfg, "slack:#eng", fixed_ts());
    seed_source(&cfg, "gmail:alice@example.com", fixed_ts());
    let resp = query_source(&cfg, None, None, None, None, &inert(), 10)
        .await
        .unwrap();
    assert_eq!(resp.hits.len(), 2);
}

#[tokio::test]
async fn query_with_time_window_filters_old_hits() {
    let (_tmp, cfg) = test_config();
    let ancient = Utc.timestamp_millis_opt(1_000_000_000_000).unwrap();
    seed_source(&cfg, "slack:#ancient", ancient);
    seed_source(&cfg, "slack:#recent", Utc::now());

    let resp = query_source(&cfg, None, None, Some(7), None, &inert(), 10)
        .await
        .unwrap();
    assert_eq!(resp.hits.len(), 1, "only the recent tree falls in 7d");
    assert_eq!(resp.hits[0].tree_scope, "slack:#recent");
}

#[tokio::test]
async fn query_truncates_to_limit() {
    let (_tmp, cfg) = test_config();
    seed_source(&cfg, "slack:#a", fixed_ts());
    seed_source(&cfg, "slack:#b", fixed_ts());
    seed_source(&cfg, "slack:#c", fixed_ts());
    let resp = query_source(&cfg, None, None, None, None, &inert(), 2)
        .await
        .unwrap();
    assert_eq!(resp.hits.len(), 2);
    assert_eq!(resp.total, 3);
    assert!(resp.truncated);
}

#[tokio::test]
async fn query_orders_newest_first() {
    let (_tmp, cfg) = test_config();
    let older = Utc::now() - Duration::hours(1);
    let newer = Utc::now();
    seed_source(&cfg, "slack:#older", older);
    seed_source(&cfg, "slack:#newer", newer);
    let resp = query_source(&cfg, None, None, None, None, &inert(), 10)
        .await
        .unwrap();
    assert_eq!(resp.hits.len(), 2);
    assert_eq!(resp.hits[0].tree_scope, "slack:#newer");
    assert_eq!(resp.hits[1].tree_scope, "slack:#older");
}

#[tokio::test]
async fn legacy_null_embedding_rows_sort_last_under_query() {
    let (_tmp, cfg) = test_config();
    seed_source(&cfg, "slack:#with-embedding", fixed_ts());
    seed_source(&cfg, "slack:#legacy-null", fixed_ts());

    // Give one summary a sidecar embedding; leave the other NULL.
    set_summary_embedding(&cfg, &summary_id_for("slack:#with-embedding"), &unit_vec(0));

    let resp = query_source(
        &cfg,
        None,
        Some(SourceKind::Chat),
        None,
        Some("any query"),
        &inert(),
        10,
    )
    .await
    .unwrap();
    assert_eq!(resp.hits.len(), 2);
    // The embedded row (even with a zero query vector → cosine 0) outranks the
    // NULL row (which sorts to the bottom).
    assert_eq!(resp.hits[0].tree_scope, "slack:#with-embedding");
    assert_eq!(resp.hits[1].tree_scope, "slack:#legacy-null");
}

#[tokio::test]
async fn sidecar_embeddings_hydrate_for_rerank() {
    let (_tmp, cfg) = test_config();
    seed_source(&cfg, "slack:#alpha", fixed_ts());
    set_summary_embedding(&cfg, &summary_id_for("slack:#alpha"), &unit_vec(0));

    // collect_source_hits must surface the sidecar vector even though the
    // in-row embedding column is NULL.
    let scored = collect_source_hits(&cfg, None, Some(SourceKind::Chat)).unwrap();
    assert_eq!(scored.len(), 1);
    assert_eq!(scored[0].1.as_deref(), Some(unit_vec(0).as_slice()));
}

#[test]
fn scope_prefix_matching_known_platforms() {
    assert!(scope_matches_kind("slack:#eng", "chat"));
    assert!(scope_matches_kind("gmail:alice", "email"));
    assert!(scope_matches_kind("notion:page123", "document"));
    assert!(scope_matches_kind("linear:conn-1:issue-abc", "document"));
    assert!(!scope_matches_kind("slack:#eng", "email"));
    assert!(scope_matches_kind("chat:custom", "chat"));
}
