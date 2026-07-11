//! Tests for the tree factory.

use super::*;
use chrono::{TimeZone, Utc};
use tempfile::TempDir;

use crate::memory::chunks::{chunk_id, upsert_chunks, Chunk, Metadata, SourceKind, SourceRef};
use crate::memory::config::MemoryConfig;
use crate::memory::tree::bucket_seal::LeafRef;
use crate::memory::tree::store::{self, TreeStatus};
use crate::memory::tree::summarise::ConcatSummariser;

fn test_config() -> (TempDir, MemoryConfig) {
    let tmp = TempDir::new().unwrap();
    let cfg = MemoryConfig::new(tmp.path());
    (tmp, cfg)
}

fn seed_chunk(cfg: &MemoryConfig, seq: u32, content: &str) -> Chunk {
    let ts = Utc
        .timestamp_millis_opt(1_700_000_000_000 + seq as i64)
        .unwrap();
    let chunk = Chunk {
        id: chunk_id(SourceKind::Chat, "slack:#eng", seq, content),
        content: content.to_string(),
        metadata: Metadata {
            source_kind: SourceKind::Chat,
            source_id: "slack:#eng".into(),
            owner: "alice".into(),
            timestamp: ts,
            time_range: (ts, ts),
            tags: vec!["eng".into()],
            source_ref: Some(SourceRef::new("slack://eng/thread")),
            path_scope: None,
        },
        token_count: 80,
        seq_in_source: seq,
        created_at: ts,
        partial_message: false,
    };
    upsert_chunks(cfg, &[chunk.clone()]).unwrap();
    chunk
}

#[test]
fn source_factory_uses_source_kind_and_full_scope() {
    let f = TreeFactory::source("slack:#eng");
    assert_eq!(f.kind(), TreeKind::Source);
    assert_eq!(f.scope(), "slack:#eng");
    assert_eq!(f.profile(), TreeProfile::Source);
}

#[test]
fn global_uses_global_scope_and_kind() {
    let g = TreeFactory::global();
    assert_eq!(g.kind(), TreeKind::Global);
    assert_eq!(g.scope(), GLOBAL_SCOPE);
}

#[test]
fn source_label_strategy_extracts_topic_empty() {
    assert!(matches!(
        TreeFactory::source("slack:#eng").label_strategy(),
        LabelStrategy::ExtractFromContent(_)
    ));
    assert!(matches!(
        TreeFactory::topic("email:alice@example.com").label_strategy(),
        LabelStrategy::Empty
    ));
}

#[test]
fn from_tree_round_trips_kind() {
    let tree = Tree {
        id: "t".into(),
        kind: TreeKind::Topic,
        scope: "person:alice".into(),
        root_id: None,
        max_level: 0,
        status: crate::memory::tree::store::TreeStatus::Active,
        created_at: chrono::Utc::now(),
        last_sealed_at: None,
    };
    let f = TreeFactory::from_tree(&tree);
    assert_eq!(f.kind(), TreeKind::Topic);
    assert_eq!(f.scope(), "person:alice");
}

#[test]
fn get_or_create_persists_tree_for_each_profile() {
    let (_tmp, cfg) = test_config();

    let source = TreeFactory::source("slack:#eng")
        .get_or_create(&cfg)
        .unwrap();
    let topic = TreeFactory::topic("person:alice")
        .get_or_create(&cfg)
        .unwrap();
    let global = TreeFactory::global().get_or_create(&cfg).unwrap();

    assert_eq!(source.kind, TreeKind::Source);
    assert_eq!(source.scope, "slack:#eng");
    assert_eq!(topic.kind, TreeKind::Topic);
    assert_eq!(topic.scope, "person:alice");
    assert_eq!(global.kind, TreeKind::Global);
    assert_eq!(global.scope, GLOBAL_SCOPE);
}

#[tokio::test]
async fn factory_insert_seal_and_archive_use_profile_scope() {
    let (_tmp, cfg) = test_config();
    let factory = TreeFactory::source("slack:#eng");
    let summariser = ConcatSummariser::new();
    let chunk = seed_chunk(&cfg, 0, "Alice is coordinating the launch plan");
    let leaf = LeafRef {
        chunk_id: chunk.id.clone(),
        token_count: cfg.tree.input_token_budget,
        timestamp: chunk.created_at,
        content: chunk.content.clone(),
        entities: vec!["person:alice".into()],
        topics: vec!["topic:launch".into()],
        score: 0.9,
    };

    let immediate = factory.insert_leaf(&cfg, &leaf, &summariser).await.unwrap();
    assert_eq!(immediate.len(), 1);

    let no_more_work = factory.seal_now(&cfg, &summariser).await.unwrap();
    assert!(no_more_work.is_empty());

    let tree = factory.get_or_create(&cfg).unwrap();
    assert_eq!(tree.root_id.as_deref(), Some(immediate[0].as_str()));
    assert_eq!(store::count_summaries(&cfg, &tree.id).unwrap(), 1);

    factory.archive(&cfg).unwrap();
    let archived = store::get_tree(&cfg, &tree.id).unwrap().unwrap();
    assert_eq!(archived.status, TreeStatus::Archived);
}

#[tokio::test]
async fn seal_now_force_seals_under_budget_l0_buffer() {
    // Regression (TR-3/TR-12): a non-empty L0 buffer that never crossed the
    // token budget must still be sealed by `seal_now` (the disconnect path).
    let (_tmp, cfg) = test_config();
    let factory = TreeFactory::source("slack:#eng");
    let summariser = ConcatSummariser::new();
    let chunk = seed_chunk(&cfg, 0, "small under-budget note");
    let leaf = LeafRef {
        chunk_id: chunk.id.clone(),
        token_count: 1, // well below input_token_budget — never auto-seals
        timestamp: chunk.created_at,
        content: chunk.content.clone(),
        entities: vec![],
        topics: vec![],
        score: 0.5,
    };

    let immediate = factory.insert_leaf(&cfg, &leaf, &summariser).await.unwrap();
    assert!(immediate.is_empty(), "under-budget leaf must not auto-seal");

    let sealed = factory.seal_now(&cfg, &summariser).await.unwrap();
    assert_eq!(sealed.len(), 1, "seal_now must force-seal the buffer");

    let tree = factory.get_or_create(&cfg).unwrap();
    assert_eq!(store::count_summaries(&cfg, &tree.id).unwrap(), 1);
    assert!(store::get_buffer(&cfg, &tree.id, 0).unwrap().is_empty());
}
