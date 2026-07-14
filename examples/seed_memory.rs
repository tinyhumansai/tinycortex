//! Seed a workspace with sample synced memory for the viewer.
//!
//! Writes a handful of [`SkillDocument`]s through the durable [`KvSkillDocSink`]
//! plus a `sync_manifest.json`, so the Next.js memory viewer can be exercised
//! end-to-end without a live Composio API key.
//!
//! ```sh
//! TINYCORTEX_WORKSPACE=/tmp/tinycortex-demo \
//!   cargo run --example seed_memory --features sync
//! ```
//!
//! If `TINYCORTEX_WORKSPACE` is unset it defaults to
//! `<tmp>/tinycortex-memory-demo`.

use serde_json::json;
use tinycortex::memory::sync::{KvSkillDocSink, SkillDocSink, SkillDocument, SKILL_DOCS_DB};

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

    println!(
        "Seeded {} documents to {} and wrote {}.",
        docs.len(),
        root.join(SKILL_DOCS_DB).display(),
        root.join("sync_manifest.json").display()
    );
    println!("Point the viewer at it:  TINYCORTEX_WORKSPACE={workspace} npm run dev");
    Ok(())
}
