//! Tests for the basic tree-walk read.

use super::*;
use chrono::{TimeZone, Utc};
use tempfile::TempDir;

use crate::memory::chunks::upsert_chunks;
use crate::memory::chunks::{Chunk, Metadata, SourceKind, SourceRef};
use crate::memory::config::MemoryConfig;
use crate::memory::tree::io::TreeReadRequest;
use crate::memory::tree::store::{self, SummaryNode, Tree, TreeKind, TreeStatus};

fn test_config() -> (TempDir, MemoryConfig) {
    let tmp = TempDir::new().unwrap();
    let cfg = MemoryConfig::new(tmp.path());
    (tmp, cfg)
}

fn ts() -> chrono::DateTime<Utc> {
    Utc.timestamp_millis_opt(1_700_000_000_000).unwrap()
}

fn summary(id: &str, level: u32, children: Vec<&str>, content: &str) -> SummaryNode {
    SummaryNode {
        id: id.into(),
        tree_id: "tree-1".into(),
        tree_kind: TreeKind::Source,
        level,
        parent_id: None,
        child_ids: children.into_iter().map(String::from).collect(),
        content: content.into(),
        token_count: 10,
        entities: vec![],
        topics: vec![],
        time_range_start: ts(),
        time_range_end: ts(),
        score: 0.5,
        sealed_at: ts(),
        deleted: false,
        embedding: None,
        doc_id: None,
        version_ms: None,
    }
}

fn chunk(id: &str, content: &str) -> Chunk {
    Chunk {
        id: id.into(),
        content: content.into(),
        metadata: Metadata {
            source_kind: SourceKind::Chat,
            source_id: "slack:#eng".into(),
            owner: "alice".into(),
            timestamp: ts(),
            time_range: (ts(), ts()),
            tags: vec![],
            source_ref: Some(SourceRef::new("slack://x")),
            path_scope: None,
        },
        token_count: 5,
        seq_in_source: 0,
        created_at: ts(),
        partial_message: false,
    }
}

fn seed_tree(cfg: &MemoryConfig) {
    let tree = Tree {
        id: "tree-1".into(),
        kind: TreeKind::Source,
        scope: "slack:#eng".into(),
        root_id: Some("l2-root".into()),
        max_level: 2,
        status: TreeStatus::Active,
        created_at: ts(),
        last_sealed_at: Some(ts()),
    };
    store::insert_tree(cfg, &tree).unwrap();
    upsert_chunks(
        cfg,
        &[chunk("chunkA", "alpha body"), chunk("chunkB", "beta body")],
    )
    .unwrap();
    crate::memory::chunks::with_connection(cfg, |conn| {
        let tx = conn.unchecked_transaction()?;
        store::insert_summary_tx(
            &tx,
            &summary("l2-root", 2, vec!["l1a", "l1b"], "root summary"),
            "t",
        )?;
        store::insert_summary_tx(
            &tx,
            &summary("l1a", 1, vec!["chunkA"], "alpha summary"),
            "t",
        )?;
        store::insert_summary_tx(&tx, &summary("l1b", 1, vec!["chunkB"], "beta summary"), "t")?;
        tx.commit()?;
        Ok(())
    })
    .unwrap();
}

#[test]
fn missing_tree_returns_empty() {
    let (_tmp, cfg) = test_config();
    let out = read_tree(
        &cfg,
        &TreeReadRequest {
            tree_id: "nope".into(),
            start_node_id: None,
            max_depth: 3,
            query: None,
            limit: None,
        },
    )
    .unwrap();
    assert!(out.hits.is_empty());
}

#[test]
fn zero_depth_returns_empty() {
    let (_tmp, cfg) = test_config();
    seed_tree(&cfg);
    let out = read_tree(
        &cfg,
        &TreeReadRequest {
            tree_id: "tree-1".into(),
            start_node_id: None,
            max_depth: 0,
            query: None,
            limit: None,
        },
    )
    .unwrap();
    assert!(out.hits.is_empty());
}

#[test]
fn full_walk_visits_summaries_and_leaves() {
    let (_tmp, cfg) = test_config();
    seed_tree(&cfg);
    let out = read_tree(
        &cfg,
        &TreeReadRequest {
            tree_id: "tree-1".into(),
            start_node_id: None,
            max_depth: 3,
            query: None,
            limit: None,
        },
    )
    .unwrap();
    let ids: std::collections::BTreeSet<&str> =
        out.hits.iter().map(|h| h.node_id.as_str()).collect();
    assert!(ids.contains("l2-root"));
    assert!(ids.contains("l1a"));
    assert!(ids.contains("l1b"));
    assert!(ids.contains("chunkA"));
    assert!(ids.contains("chunkB"));
    let leaves = out.hits.iter().filter(|h| h.node_kind == "chunk").count();
    assert_eq!(leaves, 2);
}

#[test]
fn depth_bound_limits_descent() {
    let (_tmp, cfg) = test_config();
    seed_tree(&cfg);
    // max_depth = 1: only the root node, no descent.
    let out = read_tree(
        &cfg,
        &TreeReadRequest {
            tree_id: "tree-1".into(),
            start_node_id: None,
            max_depth: 1,
            query: None,
            limit: None,
        },
    )
    .unwrap();
    assert_eq!(out.hits.len(), 1);
    assert_eq!(out.hits[0].node_id, "l2-root");
}

#[test]
fn query_scores_and_orders_by_token_overlap() {
    let (_tmp, cfg) = test_config();
    seed_tree(&cfg);
    let out = read_tree(
        &cfg,
        &TreeReadRequest {
            tree_id: "tree-1".into(),
            start_node_id: None,
            max_depth: 3,
            query: Some("alpha".into()),
            limit: None,
        },
    )
    .unwrap();
    // The "alpha"-bearing hits sort to the front with score > 0.
    assert!(out.hits[0].score > 0.0);
    assert!(out.hits[0].content.to_lowercase().contains("alpha"));
}
