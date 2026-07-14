//! Tests for compiled flavoured-tree root artifacts.

use super::*;
use chrono::{TimeZone, Utc};
use tempfile::TempDir;

use crate::memory::chunks::{chunk_id, upsert_chunks, Chunk, Metadata, SourceKind, SourceRef};
use crate::memory::config::MemoryConfig;
use crate::memory::tree::bucket_seal::LeafRef;
use crate::memory::tree::factory::TreeFactory;
use crate::memory::tree::summarise::ConcatSummariser;

const ASK: &str = "Distill the author's tweet-writing style: voice, tone, structure.";

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
        id: chunk_id(SourceKind::Chat, "tweet:alice", seq, content),
        content: content.to_string(),
        metadata: Metadata {
            source_kind: SourceKind::Chat,
            source_id: "tweet:alice".into(),
            owner: "alice".into(),
            timestamp: ts,
            time_range: (ts, ts),
            tags: vec![],
            source_ref: Some(SourceRef::new("tweet://alice/1")),
            path_scope: None,
        },
        token_count: 40,
        seq_in_source: seq,
        created_at: ts,
        partial_message: false,
    };
    upsert_chunks(cfg, std::slice::from_ref(&chunk)).unwrap();
    chunk
}

async fn seed_and_seal(cfg: &MemoryConfig, factory: &TreeFactory<'_>, n: u32) {
    let summariser = ConcatSummariser::new();
    for i in 0..n {
        let chunk = seed_chunk(cfg, i, &format!("tweet {i}: shipping is a feature."));
        let leaf = LeafRef {
            chunk_id: chunk.id.clone(),
            token_count: 1, // under budget — force-sealed below
            timestamp: chunk.created_at,
            content: chunk.content.clone(),
            entities: vec![],
            topics: vec![],
            score: 0.5,
        };
        factory.insert_leaf(cfg, &leaf, &summariser).await.unwrap();
    }
    factory.seal_now(cfg, &summariser).await.unwrap();
}

#[tokio::test]
async fn compile_root_stages_md_with_ask_and_budget() {
    let (_tmp, cfg) = test_config();
    let factory = TreeFactory::flavoured("tweet-style", ASK);
    seed_and_seal(&cfg, &factory, 3).await;

    let tree = factory.get_or_create(&cfg).unwrap();
    assert_eq!(tree.kind, TreeKind::Flavoured);
    assert_eq!(tree.ask.as_deref(), Some(ASK));
    assert!(tree.root_id.is_some(), "seal must set a root");

    let md = compile_flavoured_root(&cfg, &tree.id).unwrap();

    // Front-matter carries the ask and identity.
    assert!(md.starts_with("---\n"));
    assert!(md.contains("kind: flavoured_root"));
    assert!(md.contains(&format!("tree_id: \"{}\"", tree.id)));
    assert!(md.contains("scope: \"tweet-style\""));
    assert!(md.contains(&format!("ask: \"{ASK}\"")));
    assert!(md.contains("leaves_folded: 3"));

    // The artifact is staged at the stable, fixed path.
    let abs = flavoured_root_abs_path(&cfg, "tweet-style");
    assert!(abs.exists(), "compiled root must be written to disk");
    let on_disk = std::fs::read_to_string(&abs).unwrap();
    assert_eq!(on_disk, md);
    assert!(abs.ends_with("flavoured/tweet-style.md"));
}

#[tokio::test]
async fn compile_root_body_is_clamped_to_budget() {
    let (_tmp, mut cfg) = test_config();
    cfg.tree.flavour_root_token_budget = 5; // tiny budget forces truncation
    let factory = TreeFactory::flavoured("tiny", ASK);
    seed_and_seal(&cfg, &factory, 12).await;

    let tree = factory.get_or_create(&cfg).unwrap();
    let md = compile_flavoured_root(&cfg, &tree.id).unwrap();

    // token_estimate front-matter must respect the budget.
    let estimate: u32 = md
        .lines()
        .find_map(|l| l.strip_prefix("token_estimate: "))
        .and_then(|v| v.parse().ok())
        .expect("token_estimate present");
    assert!(estimate <= 5, "estimate {estimate} exceeds budget");
    assert!(md.contains("token_budget: 5"));
}

#[tokio::test]
async fn compile_root_rejects_non_flavoured_tree() {
    let (_tmp, cfg) = test_config();
    let source = TreeFactory::source("slack:#eng")
        .get_or_create(&cfg)
        .unwrap();
    let err = compile_flavoured_root(&cfg, &source.id).unwrap_err();
    assert!(err.to_string().contains("not flavoured"));
}

#[tokio::test]
async fn seal_auto_recompiles_root_artifact() {
    // The cascade hook should have written the artifact already, before any
    // explicit compile call.
    let (_tmp, cfg) = test_config();
    let factory = TreeFactory::flavoured("auto", ASK);
    seed_and_seal(&cfg, &factory, 2).await;

    let abs = flavoured_root_abs_path(&cfg, "auto");
    assert!(
        abs.exists(),
        "seal cascade must auto-compile the flavoured root"
    );
}

#[test]
fn rel_path_slugifies_scope() {
    assert_eq!(
        flavoured_root_rel_path("tweet:style/v1"),
        "flavoured/tweet-style-v1.md"
    );
}
