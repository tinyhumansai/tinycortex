//! Append + cascade-seal tests. Adapted from OpenHuman's `bucket_seal_tests.rs`
//! to the reduced foundation: the chat-provider summariser becomes
//! [`ConcatSummariser`] (a deterministic [`Summariser`]), chunk bodies are read
//! inline from SQLite (no on-disk staging), and the document-subtree /
//! embedding assertions are dropped (those features are deferred).

use super::*;
use crate::memory::tree::SummaryInput;
use tempfile::TempDir;

use crate::memory::chunks::upsert_chunks;
use crate::memory::chunks::{chunk_id, Chunk, Metadata, SourceKind, SourceRef};
use crate::memory::config::MemoryConfig;
use crate::memory::tree::registry::get_or_create_tree;
use crate::memory::tree::store::{self, TreeKind, INPUT_TOKEN_BUDGET, SUMMARY_FANOUT};
use crate::memory::tree::summarise::ConcatSummariser;
use chrono::{TimeZone, Utc};
use std::sync::Arc;
use tokio::sync::Notify;

struct PausingSummariser {
    started: Arc<Notify>,
    release: Arc<Notify>,
}

#[async_trait::async_trait]
impl Summariser for PausingSummariser {
    async fn summarise(
        &self,
        inputs: &[SummaryInput],
        _ctx: &SummaryContext<'_>,
    ) -> anyhow::Result<crate::memory::tree::summarise::SummaryOutput> {
        self.started.notify_one();
        self.release.notified().await;
        Ok(fallback_summary(inputs, 256))
    }
}

fn test_config() -> (TempDir, MemoryConfig) {
    let tmp = TempDir::new().unwrap();
    let cfg = MemoryConfig::new(tmp.path());
    (tmp, cfg)
}

fn mk_leaf(id: &str, tokens: u32, ts_ms: i64) -> LeafRef {
    LeafRef {
        chunk_id: id.to_string(),
        token_count: tokens,
        timestamp: Utc.timestamp_millis_opt(ts_ms).single().unwrap(),
        content: format!("content for {id}"),
        entities: vec![],
        topics: vec![],
        score: 0.5,
    }
}

/// Persist a chat chunk readable by the seal hydrator and return its id.
fn seed_chunk(
    cfg: &MemoryConfig,
    seq: u32,
    content: &str,
    tokens: u32,
    tags: Vec<String>,
) -> Chunk {
    let ts = Utc
        .timestamp_millis_opt(1_700_000_000_000 + seq as i64)
        .unwrap();
    let c = Chunk {
        id: chunk_id(SourceKind::Chat, "slack:#eng", seq, content),
        content: content.to_string(),
        metadata: Metadata {
            source_kind: SourceKind::Chat,
            source_id: "slack:#eng".into(),
            owner: "alice".into(),
            timestamp: ts,
            time_range: (ts, ts),
            tags,
            source_ref: Some(SourceRef::new("slack://x")),
            path_scope: None,
        },
        token_count: tokens,
        seq_in_source: seq,
        created_at: ts,
        partial_message: false,
    };
    upsert_chunks(cfg, std::slice::from_ref(&c)).unwrap();
    c
}

#[tokio::test]
async fn append_below_budget_does_not_seal() {
    let (_tmp, cfg) = test_config();
    let tree = get_or_create_tree(&cfg, TreeKind::Source, "slack:#eng").unwrap();
    let s = ConcatSummariser::new();
    let leaf = mk_leaf("leaf-1", 100, 1_700_000_000_000);
    let sealed = append_leaf(&cfg, &tree, &leaf, &s, &LabelStrategy::Empty)
        .await
        .unwrap();
    assert!(sealed.is_empty());

    let buf = store::get_buffer(&cfg, &tree.id, 0).unwrap();
    assert_eq!(buf.item_ids, vec!["leaf-1".to_string()]);
    assert_eq!(buf.token_sum, 100);
    assert_eq!(store::count_summaries(&cfg, &tree.id).unwrap(), 0);
}

#[tokio::test]
async fn archived_tree_rejects_new_leaf() {
    let (_tmp, cfg) = test_config();
    let tree = get_or_create_tree(&cfg, TreeKind::Source, "slack:#archived").unwrap();
    store::archive_tree(&cfg, &tree.id).unwrap();
    let summariser = ConcatSummariser::new();

    let err = append_leaf(
        &cfg,
        &tree,
        &mk_leaf("leaf-archived", 100, 1_700_000_000_000),
        &summariser,
        &LabelStrategy::Empty,
    )
    .await
    .unwrap_err();

    assert!(format!("{err:#}").contains("archived"));
    assert!(store::get_buffer(&cfg, &tree.id, 0).unwrap().is_empty());
}

#[tokio::test]
async fn archived_tree_rejects_seal_of_existing_buffer() {
    let (_tmp, cfg) = test_config();
    let tree = get_or_create_tree(&cfg, TreeKind::Source, "slack:#archived").unwrap();
    let chunk = seed_chunk(&cfg, 0, "buffered before archive", 100, vec![]);
    append_to_buffer(
        &cfg,
        &tree.id,
        0,
        &chunk.id,
        chunk.token_count as i64,
        chunk.created_at,
    )
    .unwrap();
    store::archive_tree(&cfg, &tree.id).unwrap();

    let err = cascade_all_from(
        &cfg,
        &tree,
        0,
        true,
        &ConcatSummariser::new(),
        &LabelStrategy::Empty,
    )
    .await
    .unwrap_err();
    assert!(format!("{err:#}").contains("archived"));
    assert_eq!(store::count_summaries(&cfg, &tree.id).unwrap(), 0);
}

#[tokio::test]
async fn service_seal_stages_body_and_enqueues_parent_atomically() {
    let (_tmp, mut cfg) = test_config();
    cfg.tree.summary_fanout = 1;
    let tree = get_or_create_tree(&cfg, TreeKind::Source, "slack:#eng").unwrap();
    let chunk = seed_chunk(&cfg, 0, "full staged body", 10, Vec::new());
    append_to_buffer(
        &cfg,
        &tree.id,
        0,
        &chunk.id,
        chunk.token_count as i64,
        chunk.metadata.timestamp,
    )
    .unwrap();
    let buffer = store::get_buffer(&cfg, &tree.id, 0).unwrap();
    let summariser = ConcatSummariser::new();

    let summary_id = seal_one_level_with_services(
        &cfg,
        &tree,
        &buffer,
        &SealServices {
            summariser: &summariser,
            embedder: None,
            observer: &NoopSealObserver,
        },
        &LabelStrategy::Empty,
        true,
    )
    .await
    .unwrap();

    assert!(
        crate::memory::store::content::read_summary_body(&cfg, &summary_id)
            .unwrap()
            .contains("full staged body")
    );
    assert_eq!(crate::memory::queue::count_total(&cfg).unwrap(), 1);
}

#[tokio::test]
async fn document_subtree_stages_versioned_passthrough_root() {
    let (_tmp, cfg) = test_config();
    let tree = get_or_create_tree(&cfg, TreeKind::Source, "notion:connection").unwrap();
    let chunk = seed_chunk(&cfg, 0, "document body", 10, Vec::new());
    let summariser = ConcatSummariser::new();
    let root_id = seal_document_subtree_with_services(
        &cfg,
        &tree,
        "notion:connection:page",
        Some(42),
        &[chunk.id],
        &SealServices {
            summariser: &summariser,
            embedder: None,
            observer: &NoopSealObserver,
        },
        &LabelStrategy::Empty,
    )
    .await
    .unwrap();

    let root = store::get_summary(&cfg, &root_id).unwrap().unwrap();
    assert_eq!(root.content, "document body");
    assert_eq!(root.doc_id.as_deref(), Some("notion:connection:page"));
    assert_eq!(root.version_ms, Some(42));
    assert_eq!(
        crate::memory::store::content::read_summary_body(&cfg, &root_id).unwrap(),
        "document body"
    );
    assert_eq!(
        store::get_buffer(&cfg, &tree.id, MERGE_LEVEL_BASE)
            .unwrap()
            .item_ids,
        vec![root_id]
    );
    let published = store::get_tree(&cfg, &tree.id).unwrap().unwrap();
    assert_eq!(published.root_id.as_deref(), Some(root.id.as_str()));
    assert_eq!(published.max_level, root.level);
}

#[tokio::test]
async fn crossing_budget_triggers_seal() {
    let (_tmp, cfg) = test_config();
    let tree = get_or_create_tree(&cfg, TreeKind::Source, "slack:#eng").unwrap();
    let s = ConcatSummariser::new();

    let per_leaf = INPUT_TOKEN_BUDGET * 6 / 10;
    let c1 = seed_chunk(&cfg, 0, "substantive chunk content 0", per_leaf, vec![]);
    let c2 = seed_chunk(&cfg, 1, "substantive chunk content 1", per_leaf, vec![]);
    let ts = Utc.timestamp_millis_opt(1_700_000_000_000).unwrap();

    let leaf1 = LeafRef {
        chunk_id: c1.id.clone(),
        token_count: per_leaf,
        timestamp: ts,
        content: c1.content.clone(),
        entities: vec![],
        topics: vec![],
        score: 0.5,
    };
    let leaf2 = LeafRef {
        chunk_id: c2.id.clone(),
        token_count: per_leaf,
        timestamp: ts,
        content: c2.content.clone(),
        entities: vec![],
        topics: vec![],
        score: 0.5,
    };

    let first = append_leaf(&cfg, &tree, &leaf1, &s, &LabelStrategy::Empty)
        .await
        .unwrap();
    assert!(first.is_empty());
    let second = append_leaf(&cfg, &tree, &leaf2, &s, &LabelStrategy::Empty)
        .await
        .unwrap();
    assert_eq!(second.len(), 1);

    let summary = store::get_summary(&cfg, &second[0]).unwrap().unwrap();
    assert_eq!(summary.level, 1);
    assert_eq!(summary.child_ids, vec![c1.id.clone(), c2.id.clone()]);
    assert!(summary.token_count > 0);

    assert!(store::get_buffer(&cfg, &tree.id, 0).unwrap().is_empty());
    let l1 = store::get_buffer(&cfg, &tree.id, 1).unwrap();
    assert_eq!(l1.item_ids, vec![second[0].clone()]);

    let t = store::get_tree(&cfg, &tree.id).unwrap().unwrap();
    assert_eq!(t.max_level, 1);
    assert_eq!(t.root_id.as_deref(), Some(second[0].as_str()));
    assert!(t.last_sealed_at.is_some());

    // Leaf → parent backlink populated.
    let parent: Option<String> = crate::memory::chunks::with_connection(&cfg, |conn| {
        Ok(conn
            .query_row(
                "SELECT parent_summary_id FROM mem_tree_chunks WHERE id = ?1",
                rusqlite::params![c1.id],
                |r| r.get(0),
            )
            .unwrap())
    })
    .unwrap();
    assert_eq!(parent.as_deref(), Some(second[0].as_str()));
}

#[tokio::test]
async fn seal_preserves_items_appended_while_summariser_is_running() {
    let (_tmp, cfg) = test_config();
    let tree = get_or_create_tree(&cfg, TreeKind::Source, "slack:#eng").unwrap();
    let c1 = seed_chunk(&cfg, 0, "first", 10, vec![]);
    let c2 = seed_chunk(&cfg, 1, "second", 10, vec![]);
    let ts = Utc.timestamp_millis_opt(1_700_000_000_000).unwrap();
    append_to_buffer(&cfg, &tree.id, 0, &c1.id, 10, ts).unwrap();
    let snapshot = store::get_buffer(&cfg, &tree.id, 0).unwrap();

    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let summariser = PausingSummariser {
        started: started.clone(),
        release: release.clone(),
    };
    let observer = NoopSealObserver;
    let services = SealServices {
        summariser: &summariser,
        embedder: None,
        observer: &observer,
    };
    let seal = seal_one_level_with_services(
        &cfg,
        &tree,
        &snapshot,
        &services,
        &LabelStrategy::Empty,
        false,
    );
    let append = async {
        started.notified().await;
        append_to_buffer(&cfg, &tree.id, 0, &c2.id, 10, ts).unwrap();
        release.notify_one();
    };

    let (sealed, ()) = tokio::join!(seal, append);
    sealed.unwrap();
    let remaining = store::get_buffer(&cfg, &tree.id, 0).unwrap();
    assert_eq!(remaining.item_ids, vec![c2.id]);
    assert_eq!(remaining.token_sum, 10);
}

#[tokio::test]
async fn fanout_at_l1_triggers_l2_seal() {
    let (_tmp, cfg) = test_config();
    let tree = get_or_create_tree(&cfg, TreeKind::Source, "slack:#eng").unwrap();
    let s = ConcatSummariser::new();

    let mut all_sealed: Vec<String> = Vec::new();
    for seq in 0..SUMMARY_FANOUT {
        let content = format!("substantive chunk content {seq}");
        let c = seed_chunk(&cfg, seq, &content, INPUT_TOKEN_BUDGET + 1, vec![]);
        let leaf = LeafRef {
            chunk_id: c.id.clone(),
            token_count: c.token_count,
            timestamp: Utc.timestamp_millis_opt(1_700_000_000_000).unwrap(),
            content: c.content.clone(),
            entities: vec![],
            topics: vec![],
            score: 0.5,
        };
        all_sealed.extend(
            append_leaf(&cfg, &tree, &leaf, &s, &LabelStrategy::Empty)
                .await
                .unwrap(),
        );
    }

    // fanout L1 seals + one cascading L2 seal.
    assert_eq!(all_sealed.len() as u32, SUMMARY_FANOUT + 1);
    let t = store::get_tree(&cfg, &tree.id).unwrap().unwrap();
    assert_eq!(t.max_level, 2);
    assert!(store::get_buffer(&cfg, &tree.id, 1).unwrap().is_empty());
    let l2 = store::get_buffer(&cfg, &tree.id, 2).unwrap();
    assert_eq!(l2.item_ids.len(), 1);
    let l2_summary = store::get_summary(&cfg, &l2.item_ids[0]).unwrap().unwrap();
    assert_eq!(l2_summary.level, 2);
    assert_eq!(l2_summary.child_ids.len() as u32, SUMMARY_FANOUT);
}

#[tokio::test]
async fn upper_level_does_not_seal_below_fanout() {
    let (_tmp, cfg) = test_config();
    let tree = get_or_create_tree(&cfg, TreeKind::Source, "slack:#eng").unwrap();
    let s = ConcatSummariser::new();

    let stop_before = SUMMARY_FANOUT.saturating_sub(1);
    for seq in 0..stop_before {
        let content = format!("c{seq}");
        let c = seed_chunk(&cfg, seq, &content, INPUT_TOKEN_BUDGET + 1, vec![]);
        let leaf = LeafRef {
            chunk_id: c.id.clone(),
            token_count: c.token_count,
            timestamp: Utc.timestamp_millis_opt(1_700_000_000_000).unwrap(),
            content: c.content.clone(),
            entities: vec![],
            topics: vec![],
            score: 0.5,
        };
        append_leaf(&cfg, &tree, &leaf, &s, &LabelStrategy::Empty)
            .await
            .unwrap();
    }

    let t = store::get_tree(&cfg, &tree.id).unwrap().unwrap();
    assert_eq!(t.max_level, 1, "should plateau at L1 below fanout");
    assert_eq!(
        store::get_buffer(&cfg, &tree.id, 1).unwrap().item_ids.len() as u32,
        stop_before
    );
    assert_eq!(
        store::count_summaries(&cfg, &tree.id).unwrap(),
        stop_before as u64
    );
}
