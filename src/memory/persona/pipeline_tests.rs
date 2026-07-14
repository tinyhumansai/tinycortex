//! End-to-end pipeline tests: backfill → compile, incremental skip/resume, and
//! run-budget cutoff — all on the offline mock-provider + ConcatSummariser path.

use super::*;
use async_trait::async_trait;
use std::io::Write;
use tempfile::TempDir;

use crate::memory::config::MemoryConfig;
use crate::memory::persona::config::PersonaConfig;
use crate::memory::persona::state::FileStateStore;
use crate::memory::score::extract::{ChatPrompt, ChatProvider};
use crate::memory::tree::summarise::ConcatSummariser;

struct MockChat;
#[async_trait]
impl ChatProvider for MockChat {
    fn name(&self) -> &str {
        "mock"
    }
    async fn chat_for_json(&self, _p: &ChatPrompt) -> anyhow::Result<String> {
        Ok(r#"{"observations":[
            {"facet":"workflow","observation":"Commits small and often","quote":"commit","tier":"t2"},
            {"facet":"communication","observation":"Terse and direct","quote":"do X","tier":"t2"}
        ]}"#
        .into())
    }
}

/// A provider that always hard-fails (e.g. budget exhausted / auth error).
struct FailChat;
#[async_trait]
impl ChatProvider for FailChat {
    fn name(&self) -> &str {
        "fail"
    }
    async fn chat_for_json(&self, _p: &ChatPrompt) -> anyhow::Result<String> {
        anyhow::bail!("503 upstream unavailable")
    }
}

fn user_turn(session: &str, ts: &str, text: &str) -> String {
    format!(
        r#"{{"type":"user","isSidechain":false,"cwd":"/work/demo","sessionId":"{session}","timestamp":"{ts}","message":{{"role":"user","content":"{text}"}}}}"#
    )
}

/// Build a workspace + persona config with two transcripts and one instruction
/// file. Returns (workspace tempdir, sources tempdir, MemoryConfig, PersonaConfig).
fn setup() -> (TempDir, TempDir, MemoryConfig, PersonaConfig) {
    let ws = TempDir::new().unwrap();
    let src = TempDir::new().unwrap();

    // Two Claude Code transcripts.
    let cc_root = src.path().join("claude/projects/-work-demo");
    std::fs::create_dir_all(&cc_root).unwrap();
    for (i, name) in ["a.jsonl", "b.jsonl"].iter().enumerate() {
        let mut f = std::fs::File::create(cc_root.join(name)).unwrap();
        writeln!(
            f,
            "{}",
            user_turn(
                "s1",
                &format!("2026-07-0{}T10:00:00.000Z", i + 1),
                "implement the thing and commit small"
            )
        )
        .unwrap();
    }

    // One instruction file under project_roots.
    let proj = src.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(
        proj.join("CLAUDE.md"),
        "- Always branch before writing code.\n- Commit regularly.\n",
    )
    .unwrap();

    let cfg = MemoryConfig::new(ws.path());
    let mut persona = PersonaConfig::with_home(src.path(), "me@example.com");
    persona.claude_code_root = Some(src.path().join("claude/projects"));
    persona.codex_root = None;
    persona.project_roots = vec![proj];
    persona.global_instruction_files = vec![];
    (ws, src, cfg, persona)
}

#[tokio::test]
async fn backfill_then_incremental_resume() {
    let (ws, _src, cfg, persona) = setup();
    let provider = MockChat;
    let summariser = ConcatSummariser::new();
    let store = FileStateStore::open_in_workspace(ws.path()).unwrap();

    let pipeline = Pipeline {
        config: &cfg,
        persona: &persona,
        provider: &provider,
        summariser: &summariser,
        store: &store,
    };

    // Backfill: both transcripts digested, instruction rules folded, pack written.
    let report = pipeline.run(RunMode::Backfill).await.unwrap();
    assert_eq!(report.sessions_processed, 2, "both transcripts digested");
    assert_eq!(report.directives_folded, 2, "two instruction rules");
    assert!(report.observations >= 2);
    let pack = std::fs::read_to_string(report.pack_path.as_ref().unwrap()).unwrap();
    assert!(pack.contains("# Persona: me@example.com"));
    assert!(pack.contains("## Workflow"));
    assert!(pack.contains("## Directives"));
    assert!(pack.contains("Always branch before writing code."));

    // Incremental: transcripts are cursor-skipped (no new digests). Instruction
    // files are always re-read (cheap, no LLM) so edits/removals are reflected.
    let report2 = pipeline.run(RunMode::Incremental).await.unwrap();
    assert_eq!(report2.sessions_processed, 0, "unchanged → no re-digest");
    assert!(
        report2.sessions_skipped >= 2,
        "the 2 transcripts are skipped"
    );
    // Directives are rebuilt from the re-read instruction file and still appear.
    let pack2 = std::fs::read_to_string(report2.pack_path.as_ref().unwrap()).unwrap();
    assert!(pack2.contains("Always branch before writing code."));
}

#[tokio::test]
async fn budget_cutoff_checkpoints() {
    let (ws, _src, cfg, mut persona) = setup();
    persona.run_budget.max_sessions = 1; // only one transcript may digest
    let provider = MockChat;
    let summariser = ConcatSummariser::new();
    let store = FileStateStore::open_in_workspace(ws.path()).unwrap();

    let pipeline = Pipeline {
        config: &cfg,
        persona: &persona,
        provider: &provider,
        summariser: &summariser,
        store: &store,
    };
    let report = pipeline.run(RunMode::Backfill).await.unwrap();
    assert_eq!(report.sessions_processed, 1);
    assert!(report.budget_hit, "budget should have stopped the run");
    // The pack is still compiled from what was processed (clean checkpoint).
    assert!(report.pack_path.is_some());
}

#[tokio::test]
async fn compile_only_reassembles_without_llm() {
    let (ws, _src, cfg, persona) = setup();
    let provider = MockChat;
    let summariser = ConcatSummariser::new();
    let store = FileStateStore::open_in_workspace(ws.path()).unwrap();
    let pipeline = Pipeline {
        config: &cfg,
        persona: &persona,
        provider: &provider,
        summariser: &summariser,
        store: &store,
    };
    pipeline.run(RunMode::Backfill).await.unwrap();

    // compile_only reads the existing facet-tree roots and rewrites the pack.
    let path = pipeline.compile_only().unwrap();
    let pack = std::fs::read_to_string(path).unwrap();
    assert!(pack.contains("# Persona: me@example.com"));
    assert!(pack.contains("## Directives"));
}

#[tokio::test]
async fn hard_provider_failure_does_not_commit_cursor() {
    // A hard provider failure must NOT checkpoint the transcript, so a later run
    // with a working provider re-processes it (evidence isn't lost).
    let (ws, _src, cfg, persona) = setup();
    let summariser = ConcatSummariser::new();
    let store = FileStateStore::open_in_workspace(ws.path()).unwrap();

    let failing = FailChat;
    let first = Pipeline {
        config: &cfg,
        persona: &persona,
        provider: &failing,
        summariser: &summariser,
        store: &store,
    }
    .run(RunMode::Backfill)
    .await
    .unwrap();
    assert_eq!(first.sessions_processed, 0, "all digests failed");
    assert_eq!(first.sessions_failed, 2, "both transcripts failed");
    assert_eq!(first.observations, 0);
    // Instruction directives still land (no LLM needed).
    let pack = std::fs::read_to_string(first.pack_path.as_ref().unwrap()).unwrap();
    assert!(pack.contains("Always branch before writing code."));

    // A working provider re-processes the un-committed transcripts.
    let good = MockChat;
    let second = Pipeline {
        config: &cfg,
        persona: &persona,
        provider: &good,
        summariser: &summariser,
        store: &store,
    }
    .run(RunMode::Incremental)
    .await
    .unwrap();
    assert_eq!(second.sessions_processed, 2, "failed sessions were retried");
    assert!(second.observations >= 2);
}

#[tokio::test]
async fn removed_directive_drops_out_on_rerun() {
    // Editing an instruction file (removing a rule) must drop the stale rule
    // from the pack — directives are rebuilt fresh, not appended.
    let (ws, src, cfg, persona) = setup();
    let provider = MockChat;
    let summariser = ConcatSummariser::new();
    let store = FileStateStore::open_in_workspace(ws.path()).unwrap();
    let claude_md = src.path().join("proj/CLAUDE.md");

    let first = Pipeline {
        config: &cfg,
        persona: &persona,
        provider: &provider,
        summariser: &summariser,
        store: &store,
    }
    .run(RunMode::Backfill)
    .await
    .unwrap();
    let pack1 = std::fs::read_to_string(first.pack_path.as_ref().unwrap()).unwrap();
    assert!(pack1.contains("Always branch before writing code."));
    assert!(pack1.contains("Commit regularly."));

    // Remove the "Commit regularly." rule and re-run incrementally.
    std::fs::write(&claude_md, "- Always branch before writing code.\n").unwrap();
    let second = Pipeline {
        config: &cfg,
        persona: &persona,
        provider: &provider,
        summariser: &summariser,
        store: &store,
    }
    .run(RunMode::Incremental)
    .await
    .unwrap();
    let pack2 = std::fs::read_to_string(second.pack_path.as_ref().unwrap()).unwrap();
    assert!(pack2.contains("Always branch before writing code."));
    assert!(
        !pack2.contains("Commit regularly."),
        "removed directive must not persist:\n{pack2}"
    );
}
