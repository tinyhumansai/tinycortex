//! Seed a workspace with sample synced memory for the viewer.
//!
//! Writes:
//!   1. a handful of [`SkillDocument`]s through the durable [`KvSkillDocSink`]
//!      plus a `sync_manifest.json` (the "skill docs" + "sync runs" views), and
//!   2. a real summary **memory tree** (chunks → L0 → L1 summaries) built with
//!      the offline [`ConcatSummariser`], so the graph/tree views have data.
//!
//! No live Composio API key is required.
//!
//! ```sh
//! TINYCORTEX_WORKSPACE=/tmp/tinycortex-demo \
//!   cargo run --example seed_memory --features sync
//! ```
//!
//! If `TINYCORTEX_WORKSPACE` is unset it defaults to
//! `<tmp>/tinycortex-memory-demo`.

use chrono::{TimeZone, Utc};
use serde_json::json;

use tinycortex::memory::chunks::{chunk_id, upsert_chunks, Chunk, Metadata, SourceKind, SourceRef};
use tinycortex::memory::config::MemoryConfig;
use tinycortex::memory::sync::{KvSkillDocSink, SkillDocSink, SkillDocument, SKILL_DOCS_DB};
use tinycortex::memory::tree::{ConcatSummariser, LeafRef, TreeFactory};

// ── Skill documents (the "skill docs" + "sync runs" views) ──────────────────

fn doc(toolkit: &str, id: &str, title: &str, content: &str) -> SkillDocument {
    SkillDocument {
        namespace_skill_id: toolkit.into(),
        connection_id: format!("ca_demo_{toolkit}"),
        document_id: format!("{toolkit}:{id}"),
        title: title.into(),
        content: content.into(),
        toolkit: toolkit.into(),
        metadata: json!({
            "source": "composio-provider-incremental",
            "taint": "external_sync",
            "provider_id": id,
        }),
    }
}

async fn seed_skill_docs(root: &std::path::Path) -> anyhow::Result<usize> {
    let docs = [
        doc(
            "gmail",
            "18f2a1",
            "Re: Q3 planning sync",
            "Thanks for the notes. Let's lock the roadmap review for Thursday and \
             pull the metrics dashboard into the deck beforehand.",
        ),
        doc(
            "gmail",
            "18f2b7",
            "Invoice #4021 from Acme Cloud",
            "Your monthly invoice is attached. Amount due: $412.00. \
             Auto-pay will run on the 5th.",
        ),
        doc(
            "github",
            "pr-882",
            "harden queue and tree concurrency (#882)",
            "Adds a busy-timeout to the chunk pool and serializes tree seals. \
             Fixes intermittent SQLITE_BUSY under parallel ingest.",
        ),
        doc(
            "github",
            "issue-66",
            "port OpenHuman engine gaps",
            "Tracking parity work: entity index, summary fan-out, and the \
             periodic sync cadence.",
        ),
        doc(
            "linear",
            "TIN-140",
            "Build memory debug viewer",
            "Next.js app that inspects the local workspace: skill docs, the \
             memory tree, and sync run manifests.",
        ),
    ];

    let sink = KvSkillDocSink::open_in_workspace(root)?;
    for document in docs.iter().cloned() {
        sink.store(document).await?;
    }

    let manifest = json!({
        "toolkits": [
            { "toolkit": "gmail", "connectionId": "ca_demo_gmail", "ingested": 2,
              "actions": 2, "costUsd": 0.0, "docsStored": 2, "taintOk": true,
              "cursorAdvanced": true, "idempotency": "PASS", "passed": true, "error": null },
            { "toolkit": "github", "connectionId": "ca_demo_github", "ingested": 2,
              "actions": 3, "costUsd": 0.0, "docsStored": 2, "taintOk": true,
              "cursorAdvanced": true, "idempotency": "PASS", "passed": true, "error": null },
            { "toolkit": "linear", "connectionId": "ca_demo_linear", "ingested": 1,
              "actions": 1, "costUsd": 0.0, "docsStored": 1, "taintOk": true,
              "cursorAdvanced": true, "idempotency": "PASS", "passed": true, "error": null }
        ],
        "events": [
            { "sourceId": "gmail", "toolkit": "gmail", "connectionId": "ca_demo_gmail", "stage": "completed" },
            { "sourceId": "github", "toolkit": "github", "connectionId": "ca_demo_github", "stage": "completed" },
            { "sourceId": "linear", "toolkit": "linear", "connectionId": "ca_demo_linear", "stage": "completed" }
        ],
        "documentsPersisted": docs.len(),
    });
    std::fs::create_dir_all(root)?;
    std::fs::write(
        root.join("sync_manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    Ok(docs.len())
}

// ── Memory tree (the "graph" + "memory tree" views) ─────────────────────────

/// One leaf message: (content, entities, topics).
type Message = (
    &'static str,
    &'static [&'static str],
    &'static [&'static str],
);

/// Seed one source tree: persist a chunk per message, insert each as a leaf,
/// then force-seal. A high per-leaf token count seals every leaf into its own
/// L0 summary, and once `SUMMARY_FANOUT` L0s accumulate an L1 seals — producing
/// a visible chunk → L0 → L1 hierarchy.
async fn seed_tree(
    cfg: &MemoryConfig,
    kind: SourceKind,
    scope: &str,
    owner: &str,
    messages: &[Message],
) -> anyhow::Result<()> {
    let factory = TreeFactory::source(scope);
    let summariser = ConcatSummariser::new();
    let base_ms = 1_700_000_000_000_i64;

    for (seq, (content, entities, topics)) in messages.iter().enumerate() {
        let ts = Utc
            .timestamp_millis_opt(base_ms + seq as i64 * 60_000)
            .single()
            .expect("valid timestamp");
        let chunk = Chunk {
            id: chunk_id(kind, scope, seq as u32, content),
            content: (*content).to_string(),
            metadata: Metadata {
                source_kind: kind,
                source_id: scope.to_string(),
                owner: owner.to_string(),
                timestamp: ts,
                time_range: (ts, ts),
                tags: vec![scope.split(':').next().unwrap_or("source").to_string()],
                source_ref: Some(SourceRef::new(format!("{scope}/{seq}"))),
                path_scope: None,
            },
            token_count: 80,
            seq_in_source: seq as u32,
            created_at: ts,
            partial_message: false,
        };
        upsert_chunks(cfg, std::slice::from_ref(&chunk))?;

        let leaf = LeafRef {
            chunk_id: chunk.id,
            // >= input_token_budget so each leaf seals into its own L0.
            token_count: cfg.tree.input_token_budget,
            timestamp: ts,
            content: (*content).to_string(),
            entities: entities.iter().map(|e| e.to_string()).collect(),
            topics: topics.iter().map(|t| t.to_string()).collect(),
            score: 0.8,
        };
        factory.insert_leaf(cfg, &leaf, &summariser).await?;
    }
    factory.seal_now(cfg, &summariser).await?;
    Ok(())
}

async fn seed_trees(root: &std::path::Path) -> anyhow::Result<()> {
    let cfg = MemoryConfig::new(root);

    let gmail: &[Message] = &[
        (
            "Kicking off Q3 planning — please send roadmap inputs by Friday.",
            &["person:alice", "topic:planning"],
            &["topic:planning"],
        ),
        (
            "Roadmap review moved to Thursday; metrics dashboard added to the deck.",
            &["person:alice", "person:bob"],
            &["topic:planning"],
        ),
        (
            "Acme Cloud invoice #4021 — $412 due on the 5th, auto-pay enabled.",
            &["org:acme"],
            &["topic:billing"],
        ),
        (
            "Re: budget — engineering headcount approved for two hires.",
            &["person:carol"],
            &["topic:budget"],
        ),
        (
            "Offsite logistics: booking the venue for the last week of August.",
            &["person:bob"],
            &["topic:offsite"],
        ),
        (
            "Security review scheduled — please rotate the staging credentials.",
            &["person:dave", "topic:security"],
            &["topic:security"],
        ),
        (
            "Customer escalation from Globex resolved; postmortem attached.",
            &["org:globex"],
            &["topic:support"],
        ),
        (
            "Weekly metrics: signups up 12%, churn flat, NPS at 41.",
            &["topic:metrics"],
            &["topic:metrics"],
        ),
        (
            "Design handoff for the memory viewer is ready for review.",
            &["person:erin", "topic:design"],
            &["topic:design"],
        ),
        (
            "Reminder: submit expense reports before end of month.",
            &["topic:ops"],
            &["topic:ops"],
        ),
        (
            "Re: hiring — onsite loop for the platform role is confirmed.",
            &["person:carol"],
            &["topic:hiring"],
        ),
    ];

    let github: &[Message] = &[
        (
            "PR #882: harden queue and tree concurrency, add busy-timeout.",
            &["repo:tinycortex", "topic:concurrency"],
            &["topic:concurrency"],
        ),
        (
            "Issue #66: port OpenHuman engine gaps — entity index and fan-out.",
            &["repo:tinycortex"],
            &["topic:parity"],
        ),
        (
            "PR #884: durable KvSkillDocSink for inspectable synced memory.",
            &["repo:tinycortex", "topic:sync"],
            &["topic:sync"],
        ),
        (
            "Issue #71: intermittent SQLITE_BUSY under parallel ingest.",
            &["topic:bug"],
            &["topic:bug"],
        ),
        (
            "PR #889: Next.js memory viewer reading the workspace server-side.",
            &["repo:tinycortex", "topic:viewer"],
            &["topic:viewer"],
        ),
        (
            "Issue #73: document the on-disk layout of the memory tree.",
            &["topic:docs"],
            &["topic:docs"],
        ),
        (
            "PR #891: force-directed graph view of the summary tree.",
            &["topic:viewer", "topic:graph"],
            &["topic:graph"],
        ),
        (
            "Issue #75: entity co-occurrence edges are not populated on insert.",
            &["topic:entities"],
            &["topic:entities"],
        ),
        (
            "PR #893: seed example builds a real tree for demos.",
            &["repo:tinycortex"],
            &["topic:demo"],
        ),
        (
            "Issue #77: Composio connect flow for new account ids.",
            &["topic:composio"],
            &["topic:composio"],
        ),
        (
            "PR #895: load .env in the composio harness.",
            &["topic:composio"],
            &["topic:config"],
        ),
    ];

    seed_tree(&cfg, SourceKind::Email, "gmail:inbox", "alice", gmail).await?;
    seed_tree(&cfg, SourceKind::Chat, "github:tinycortex", "bot", github).await?;
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let workspace = std::env::var("TINYCORTEX_WORKSPACE")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| {
            std::env::temp_dir()
                .join("tinycortex-memory-demo")
                .to_string_lossy()
                .into_owned()
        });
    let root = std::path::Path::new(&workspace);

    let doc_count = seed_skill_docs(root).await?;
    seed_trees(root).await?;

    println!(
        "Seeded {doc_count} skill documents to {} and built a memory tree in {}.",
        root.join(SKILL_DOCS_DB).display(),
        root.join("memory_tree/chunks.db").display()
    );
    println!("Point the viewer at it:  TINYCORTEX_WORKSPACE={workspace} npm run dev");
    Ok(())
}
