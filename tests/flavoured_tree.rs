//! Integration test for ask-driven flavoured trees (issue #68).
//!
//! Seeds N style-evidence leaves into a flavoured tree, seals, and asserts the
//! compiled root artifact exists on disk, stays within the token budget, and
//! carries the ask in its front-matter.

use chrono::{TimeZone, Utc};
use tempfile::TempDir;

use tinycortex::memory::chunks::{chunk_id, upsert_chunks, Chunk, Metadata, SourceKind, SourceRef};
use tinycortex::memory::config::MemoryConfig;
use tinycortex::memory::tree::bucket_seal::LeafRef;
use tinycortex::memory::tree::factory::TreeFactory;
use tinycortex::memory::tree::flavoured::{compile_flavoured_root, flavoured_root_abs_path};
use tinycortex::memory::tree::store::TreeKind;
use tinycortex::memory::tree::summarise::ConcatSummariser;

const ASK: &str = "Distill the author's tweet-writing style: voice, tone, \
                   structure, vocabulary, and concrete dos and don'ts.";

const EVIDENCE: &[&str] = &[
    "ship early, ship often — a demo beats a deck",
    "lowercase energy. no exclamation points, ever",
    "one idea per tweet. cut the qualifier",
    "concrete numbers > adjectives. '3x faster' not 'much faster'",
    "end on a hook, not a summary",
    "prefer verbs. delete 'really', 'very', 'just'",
];

fn seed_leaf(cfg: &MemoryConfig, seq: u32, content: &str) -> LeafRef {
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
            source_ref: Some(SourceRef::new("tweet://alice")),
            path_scope: None,
        },
        token_count: 20,
        seq_in_source: seq,
        created_at: ts,
        partial_message: false,
    };
    upsert_chunks(cfg, std::slice::from_ref(&chunk)).unwrap();
    LeafRef {
        chunk_id: chunk.id,
        token_count: 1, // under budget — force-sealed via seal_now
        timestamp: ts,
        content: content.to_string(),
        entities: vec![],
        topics: vec![],
        score: 0.7,
    }
}

#[tokio::test]
async fn flavoured_tree_compiles_prompt_ready_root() {
    let tmp = TempDir::new().unwrap();
    let cfg = MemoryConfig::new(tmp.path());
    let summariser = ConcatSummariser::new();

    let factory = TreeFactory::flavoured("tweet-style", ASK);
    for (i, evidence) in EVIDENCE.iter().enumerate() {
        let leaf = seed_leaf(&cfg, i as u32, evidence);
        factory.insert_leaf(&cfg, &leaf, &summariser).await.unwrap();
    }
    // Force-seal the under-budget L0 buffer (the "compile the profile now" path).
    factory.seal_now(&cfg, &summariser).await.unwrap();

    let tree = factory.get_or_create(&cfg).unwrap();
    assert_eq!(tree.kind, TreeKind::Flavoured);
    assert_eq!(tree.ask.as_deref(), Some(ASK));
    assert!(tree.root_id.is_some(), "seal must produce a root");

    let md = compile_flavoured_root(&cfg, &tree.id).unwrap();

    // 1. Compiled root exists on the fixed, host-readable path.
    let abs = flavoured_root_abs_path(&cfg, "tweet-style");
    assert!(abs.exists());
    assert_eq!(std::fs::read_to_string(&abs).unwrap(), md);

    // 2. Front-matter carries the ask (as a one-line escaped scalar) and identity.
    let expected_ask = ASK.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(md.contains(&format!("ask: \"{expected_ask}\"")));
    assert!(md.contains("kind: flavoured_root"));
    assert!(md.contains("scope: \"tweet-style\""));
    assert!(md.contains(&format!("leaves_folded: {}", EVIDENCE.len())));

    // 3. Body stays within the default 1000-token budget.
    let estimate: u32 = md
        .lines()
        .find_map(|l| l.strip_prefix("token_estimate: "))
        .and_then(|v| v.parse().ok())
        .expect("token_estimate present");
    assert!(estimate <= cfg.tree.flavour_root_token_budget);

    // 4. The evidence actually made it into the profile body.
    let body = md.split("---\n").nth(2).unwrap_or_default();
    assert!(body.contains("ship early"));
}
