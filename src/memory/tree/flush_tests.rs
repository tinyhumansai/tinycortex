//! Tests for time-based buffer flush. Adapted from OpenHuman's flush tests:
//! the chat-provider summariser is replaced by [`ConcatSummariser`], and chunks
//! are read inline (no on-disk content staging).

use super::*;
use chrono::Duration;
use tempfile::TempDir;

use crate::memory::chunks::upsert_chunks;
use crate::memory::chunks::{chunk_id, Chunk, Metadata, SourceKind, SourceRef};
use crate::memory::tree::bucket_seal::{append_leaf, LeafRef};
use crate::memory::tree::registry::get_or_create_tree;
use crate::memory::tree::store::{upsert_buffer_tx, Buffer, TreeKind};
use crate::memory::tree::summarise::ConcatSummariser;

fn test_config() -> (TempDir, MemoryConfig) {
    let tmp = TempDir::new().unwrap();
    let cfg = MemoryConfig::new(tmp.path());
    (tmp, cfg)
}

fn seed_chunk(cfg: &MemoryConfig, src: &str, seq: u32, content: &str, ts: DateTime<Utc>) -> Chunk {
    let c = Chunk {
        id: chunk_id(SourceKind::Chat, src, seq, content),
        content: content.to_string(),
        metadata: Metadata {
            source_kind: SourceKind::Chat,
            source_id: src.into(),
            owner: "alice".into(),
            timestamp: ts,
            time_range: (ts, ts),
            tags: vec![],
            source_ref: Some(SourceRef::new("slack://x")),
            path_scope: None,
        },
        token_count: 100,
        seq_in_source: seq,
        created_at: ts,
        partial_message: false,
    };
    upsert_chunks(cfg, &[c.clone()]).unwrap();
    c
}

#[tokio::test]
async fn flush_seals_old_buffer_even_under_budget() {
    let (_tmp, cfg) = test_config();
    let tree = get_or_create_tree(&cfg, TreeKind::Source, "slack:#eng").unwrap();
    let s = ConcatSummariser::new();
    let old_ts = Utc::now() - Duration::days(10);
    let c = seed_chunk(&cfg, "slack:#eng", 0, "old content to seal", old_ts);

    let leaf = LeafRef {
        chunk_id: c.id.clone(),
        token_count: 100,
        timestamp: old_ts,
        content: c.content.clone(),
        entities: vec![],
        topics: vec![],
        score: 0.5,
    };
    append_leaf(&cfg, &tree, &leaf, &s, &LabelStrategy::Empty)
        .await
        .unwrap();
    assert_eq!(store::count_summaries(&cfg, &tree.id).unwrap(), 0);

    let seals = flush_stale_buffers(&cfg, Duration::days(7), &s, &LabelStrategy::Empty)
        .await
        .unwrap();
    assert_eq!(seals, 1);
    assert_eq!(store::count_summaries(&cfg, &tree.id).unwrap(), 1);
    assert!(store::get_buffer(&cfg, &tree.id, 0).unwrap().is_empty());
}

#[tokio::test]
async fn flush_does_not_force_seal_under_fanout_upper_buffer() {
    let (_tmp, cfg) = test_config();
    let tree = get_or_create_tree(&cfg, TreeKind::Source, "slack:#eng").unwrap();
    let s = ConcatSummariser::new();

    let old_ts = Utc::now() - Duration::days(10);
    crate::memory::chunks::with_connection(&cfg, |conn| {
        let tx = conn.unchecked_transaction()?;
        upsert_buffer_tx(
            &tx,
            &Buffer {
                tree_id: tree.id.clone(),
                level: 1,
                item_ids: vec!["fake-l1-child".into()],
                token_sum: 100,
                oldest_at: Some(old_ts),
            },
        )?;
        tx.commit()?;
        Ok(())
    })
    .unwrap();

    let seals = flush_stale_buffers(&cfg, Duration::days(7), &s, &LabelStrategy::Empty)
        .await
        .unwrap();
    assert_eq!(seals, 0, "L1 stale buffer must not be force-sealed");
    assert_eq!(store::count_summaries(&cfg, &tree.id).unwrap(), 0);
    assert_eq!(
        store::get_buffer(&cfg, &tree.id, 1).unwrap().item_ids,
        vec!["fake-l1-child".to_string()]
    );
}

#[tokio::test]
async fn flush_noop_when_buffer_is_recent() {
    let (_tmp, cfg) = test_config();
    let tree = get_or_create_tree(&cfg, TreeKind::Source, "slack:#eng").unwrap();
    let s = ConcatSummariser::new();
    let now = Utc::now();
    let c = seed_chunk(&cfg, "slack:#eng", 0, "fresh", now);
    let leaf = LeafRef {
        chunk_id: c.id.clone(),
        token_count: 50,
        timestamp: now,
        content: c.content.clone(),
        entities: vec![],
        topics: vec![],
        score: 0.5,
    };
    append_leaf(&cfg, &tree, &leaf, &s, &LabelStrategy::Empty)
        .await
        .unwrap();

    let seals = flush_stale_buffers(&cfg, Duration::days(7), &s, &LabelStrategy::Empty)
        .await
        .unwrap();
    assert_eq!(seals, 0);
    assert_eq!(store::count_summaries(&cfg, &tree.id).unwrap(), 0);
}

#[tokio::test]
async fn flush_seals_multiple_distinct_trees_via_batched_lookup() {
    let (_tmp, cfg) = test_config();
    let tree_a = get_or_create_tree(&cfg, TreeKind::Source, "slack:#eng").unwrap();
    let tree_b = get_or_create_tree(&cfg, TreeKind::Source, "slack:#design").unwrap();
    assert_ne!(tree_a.id, tree_b.id);
    let s = ConcatSummariser::new();
    let old_ts = Utc::now() - Duration::days(10);

    for (src, content, tree) in [
        ("slack:#eng", "engineering content", &tree_a),
        ("slack:#design", "design content", &tree_b),
    ] {
        let c = seed_chunk(&cfg, src, 0, content, old_ts);
        let leaf = LeafRef {
            chunk_id: c.id.clone(),
            token_count: 100,
            timestamp: old_ts,
            content: c.content.clone(),
            entities: vec![],
            topics: vec![],
            score: 0.5,
        };
        append_leaf(&cfg, tree, &leaf, &s, &LabelStrategy::Empty)
            .await
            .unwrap();
    }

    let seals = flush_stale_buffers(&cfg, Duration::days(7), &s, &LabelStrategy::Empty)
        .await
        .unwrap();
    assert_eq!(seals, 2);
    assert_eq!(store::count_summaries(&cfg, &tree_a.id).unwrap(), 1);
    assert_eq!(store::count_summaries(&cfg, &tree_b.id).unwrap(), 1);
}

#[tokio::test]
async fn flush_default_noops_when_no_stale_buffers_exist() {
    let (_tmp, cfg) = test_config();
    let s = ConcatSummariser::new();

    let seals = flush_stale_buffers_default(&cfg, &s, &LabelStrategy::Empty)
        .await
        .unwrap();

    assert_eq!(seals, 0);
}

#[tokio::test]
async fn force_flush_tree_reports_missing_tree() {
    let (_tmp, cfg) = test_config();
    let s = ConcatSummariser::new();

    let err = force_flush_tree(&cfg, "missing-tree", None, &s, &LabelStrategy::Empty)
        .await
        .expect_err("missing tree should be reported");

    assert!(
        format!("{err:#}").contains("no tree with id missing-tree"),
        "unexpected error: {err:#}"
    );
}

#[tokio::test]
async fn force_flush_tree_seals_current_l0_buffer() {
    let (_tmp, cfg) = test_config();
    let tree = get_or_create_tree(&cfg, TreeKind::Source, "slack:#eng").unwrap();
    let s = ConcatSummariser::new();
    let now = Utc::now();
    let c = seed_chunk(&cfg, "slack:#eng", 0, "manual flush content", now);
    let leaf = LeafRef {
        chunk_id: c.id.clone(),
        token_count: 50,
        timestamp: now,
        content: c.content.clone(),
        entities: vec![],
        topics: vec![],
        score: 0.5,
    };
    append_leaf(&cfg, &tree, &leaf, &s, &LabelStrategy::Empty)
        .await
        .unwrap();

    let sealed = force_flush_tree(&cfg, &tree.id, Some(now), &s, &LabelStrategy::Empty)
        .await
        .unwrap();

    assert_eq!(sealed.len(), 1);
    assert!(store::get_buffer(&cfg, &tree.id, 0).unwrap().is_empty());
    assert_eq!(store::count_summaries(&cfg, &tree.id).unwrap(), 1);
}
