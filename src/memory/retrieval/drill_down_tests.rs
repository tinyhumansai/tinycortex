//! `drill_down` tests. Trees, summaries, and leaf chunks are seeded directly.

use super::*;
use crate::memory::chunks::{chunk_id, Chunk, Metadata, SourceKind, SourceRef};
use crate::memory::config::MemoryConfig;
use crate::memory::retrieval::test_support::{
    fixed_ts, insert_chunks, insert_summary, insert_tree_row, source_tree, summary_node,
    test_config,
};
use crate::memory::score::embed::InertEmbedder;
use crate::memory::tree::{SummaryNode, TreeKind};
use chrono::{DateTime, Utc};

fn inert() -> InertEmbedder {
    InertEmbedder::new()
}

fn leaf_chunk(source: &str, seq: u32, content: &str, ts: DateTime<Utc>) -> Chunk {
    Chunk {
        id: chunk_id(SourceKind::Chat, source, seq, content),
        content: content.to_string(),
        metadata: Metadata {
            source_kind: SourceKind::Chat,
            source_id: source.into(),
            owner: "alice".into(),
            timestamp: ts,
            time_range: (ts, ts),
            tags: vec![],
            source_ref: Some(SourceRef::new("slack://x")),
            path_scope: None,
        },
        token_count: 10,
        seq_in_source: seq,
        created_at: ts,
        partial_message: false,
    }
}

/// Seed a single-level tree: one L1 summary over two leaf chunks. Returns
/// `(summary_id, first_leaf_id)`.
fn seed_one_level(cfg: &MemoryConfig) -> (String, String) {
    let ts = fixed_ts();
    let a = leaf_chunk("slack:#eng", 0, "leaf-0", ts);
    let b = leaf_chunk("slack:#eng", 1, "leaf-1", ts);
    insert_chunks(cfg, &[a.clone(), b.clone()]);
    let tree = source_tree("tree:eng", "slack:#eng", Some("s:L1"), 1);
    insert_tree_row(cfg, &tree);
    let node = summary_node("s:L1", "tree:eng", 1, None, &[&a.id, &b.id], "L1", ts);
    insert_summary(cfg, &node);
    ("s:L1".into(), a.id)
}

#[tokio::test]
async fn depth_zero_returns_empty() {
    let (_tmp, cfg) = test_config();
    let (root, _) = seed_one_level(&cfg);
    let out = drill_down(&cfg, &root, 0, None, &inert(), None)
        .await
        .unwrap();
    assert!(out.is_empty());
}

#[tokio::test]
async fn invalid_id_returns_empty() {
    let (_tmp, cfg) = test_config();
    let out = drill_down(&cfg, "nonexistent:id", 1, None, &inert(), None)
        .await
        .unwrap();
    assert!(out.is_empty());
}

#[tokio::test]
async fn summary_drills_to_leaves_at_depth_one() {
    let (_tmp, cfg) = test_config();
    let (root, _) = seed_one_level(&cfg);
    let out = drill_down(&cfg, &root, 1, None, &inert(), None)
        .await
        .unwrap();
    assert_eq!(out.len(), 2, "L1 has 2 leaf children");
    for hit in &out {
        assert_eq!(hit.level, 0, "direct children of L1 are leaves");
    }
}

#[tokio::test]
async fn leaf_drill_down_returns_empty() {
    let (_tmp, cfg) = test_config();
    let (_root, leaf) = seed_one_level(&cfg);
    let out = drill_down(&cfg, &leaf, 3, None, &inert(), None)
        .await
        .unwrap();
    assert!(out.is_empty(), "leaves have no children");
}

#[tokio::test]
async fn deeper_max_depth_does_not_break_on_shallow_tree() {
    let (_tmp, cfg) = test_config();
    let (root, _) = seed_one_level(&cfg);
    let out = drill_down(&cfg, &root, 5, None, &inert(), None)
        .await
        .unwrap();
    assert_eq!(out.len(), 2);
}

#[tokio::test]
async fn query_with_limit_truncates_after_rerank() {
    let (_tmp, cfg) = test_config();
    let (root, _) = seed_one_level(&cfg);
    let out = drill_down(&cfg, &root, 1, Some("phoenix"), &inert(), Some(1))
        .await
        .unwrap();
    assert_eq!(out.len(), 1, "limit=1 truncates 2 children to 1");
}

#[tokio::test]
async fn query_without_limit_returns_all_children() {
    let (_tmp, cfg) = test_config();
    let (root, _) = seed_one_level(&cfg);
    let out = drill_down(&cfg, &root, 1, Some("phoenix"), &inert(), None)
        .await
        .unwrap();
    assert_eq!(out.len(), 2);
}

/// Build a 2-level tree directly: L2 root → [L1_a, L1_b]; each L1 over 2 leaves.
fn seed_two_level(cfg: &MemoryConfig) -> (String, Vec<String>, Vec<String>) {
    let ts = fixed_ts();
    let leaves: Vec<Chunk> = (0..4)
        .map(|i| leaf_chunk("slack:#eng", i, &format!("leaf-{i}"), ts))
        .collect();
    insert_chunks(cfg, &leaves);
    let leaf_ids: Vec<String> = leaves.iter().map(|c| c.id.clone()).collect();

    let tree = source_tree("tree:eng", "slack:#eng", Some("s:L2"), 2);
    insert_tree_row(cfg, &tree);

    let l1_a = summary_node(
        "s:L1:a",
        "tree:eng",
        1,
        Some("s:L2"),
        &[&leaf_ids[0], &leaf_ids[1]],
        "L1 A",
        ts,
    );
    let l1_b = summary_node(
        "s:L1:b",
        "tree:eng",
        1,
        Some("s:L2"),
        &[&leaf_ids[2], &leaf_ids[3]],
        "L1 B",
        ts,
    );
    let root = summary_node("s:L2", "tree:eng", 2, None, &["s:L1:a", "s:L1:b"], "L2", ts);
    insert_summary(cfg, &l1_a);
    insert_summary(cfg, &l1_b);
    insert_summary(cfg, &root);

    (
        "s:L2".into(),
        vec!["s:L1:a".into(), "s:L1:b".into()],
        leaf_ids,
    )
}

#[tokio::test]
async fn walk_visits_siblings_before_descendants_bfs_order() {
    let (_tmp, cfg) = test_config();
    let (root, l1_ids, leaf_ids) = seed_two_level(&cfg);

    let out = drill_down(&cfg, &root, 2, None, &inert(), None)
        .await
        .unwrap();
    assert_eq!(out.len(), 6, "2×L1 + 4 leaves");
    let ordered: Vec<&str> = out.iter().map(|h| h.node_id.as_str()).collect();
    let last_l1 = l1_ids
        .iter()
        .map(|id| ordered.iter().position(|&n| n == id).unwrap())
        .max()
        .unwrap();
    let first_leaf = leaf_ids
        .iter()
        .map(|id| ordered.iter().position(|&n| n == id).unwrap())
        .min()
        .unwrap();
    assert!(
        last_l1 < first_leaf,
        "BFS: both L1 before any leaf; got {ordered:?}"
    );
}

#[tokio::test]
async fn per_depth_batch_keys_hit_scope_by_tree_id_not_position() {
    let (_tmp, cfg) = test_config();
    let ts = fixed_ts();
    let tree_a = source_tree("tree-a", "slack:#eng", Some("s:root"), 2);
    let tree_b = source_tree("tree-b", "slack:#design", None, 2);
    insert_tree_row(&cfg, &tree_a);
    insert_tree_row(&cfg, &tree_b);

    let l1_a = summary_node("s:L1:a", "tree-a", 1, Some("s:root"), &[], "from a", ts);
    let l1_b = summary_node("s:L1:b", "tree-b", 1, Some("s:root"), &[], "from b", ts);
    let root = summary_node(
        "s:root",
        "tree-a",
        2,
        None,
        &["s:L1:a", "s:L1:b"],
        "root",
        ts,
    );
    insert_summary(&cfg, &l1_a);
    insert_summary(&cfg, &l1_b);
    insert_summary(&cfg, &root);

    let out = drill_down(&cfg, "s:root", 1, None, &inert(), None)
        .await
        .unwrap();
    assert_eq!(out.len(), 2);
    let hit_a = out.iter().find(|h| h.node_id == "s:L1:a").unwrap();
    let hit_b = out.iter().find(|h| h.node_id == "s:L1:b").unwrap();
    assert_eq!(hit_a.tree_scope, "slack:#eng");
    assert_eq!(hit_b.tree_scope, "slack:#design");
    assert_eq!(hit_a.tree_id, "tree-a");
    assert_eq!(hit_b.tree_id, "tree-b");
}

#[tokio::test]
async fn drill_down_surfaces_only_latest_doc_version() {
    let (_tmp, cfg) = test_config();
    let ts = fixed_ts();
    let chunk_v1 = leaf_chunk("notion:pageA", 0, "old body", ts);
    let chunk_v2 = leaf_chunk("notion:pageA", 1, "new body", ts);
    insert_chunks(&cfg, &[chunk_v1.clone(), chunk_v2.clone()]);

    let tree = source_tree("tree:notion", "notion:conn1", Some("s:merge"), 1000);
    insert_tree_row(&cfg, &tree);

    let mk_root = |id: &str, version: i64, child: &str| -> SummaryNode {
        let mut n = summary_node(id, "tree:notion", 1, Some("s:merge"), &[child], "doc", ts);
        n.tree_kind = TreeKind::Source;
        n.doc_id = Some("notion:conn1:pageA".into());
        n.version_ms = Some(version);
        n
    };
    let v1 = mk_root("s:v1", 100, &chunk_v1.id);
    let v2 = mk_root("s:v2", 200, &chunk_v2.id);
    let merge = summary_node(
        "s:merge",
        "tree:notion",
        1000,
        None,
        &["s:v1", "s:v2"],
        "merge",
        ts,
    );
    insert_summary(&cfg, &v1);
    insert_summary(&cfg, &v2);
    insert_summary(&cfg, &merge);

    let out = drill_down(&cfg, "s:merge", 3, None, &inert(), None)
        .await
        .unwrap();
    let ids: Vec<&str> = out.iter().map(|h| h.node_id.as_str()).collect();
    assert!(
        ids.contains(&"s:v2"),
        "latest doc-root must surface; got {ids:?}"
    );
    assert!(
        ids.contains(&chunk_v2.id.as_str()),
        "latest chunk must surface"
    );
    assert!(!ids.contains(&"s:v1"), "superseded doc-root filtered");
    assert!(
        !ids.contains(&chunk_v1.id.as_str()),
        "superseded chunk filtered"
    );
}
