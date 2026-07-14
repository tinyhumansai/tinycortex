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
            user_turn("s1", &format!("2026-07-0{}T10:00:00.000Z", i + 1), "implement the thing and commit small")
        )
        .unwrap();
    }

    // One instruction file under project_roots.
    let proj = src.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(proj.join("CLAUDE.md"), "- Always branch before writing code.\n- Commit regularly.\n").unwrap();

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

    // Incremental: nothing changed → everything skipped, no new digests.
    let report2 = pipeline.run(RunMode::Incremental).await.unwrap();
    assert_eq!(report2.sessions_processed, 0, "unchanged → no re-digest");
    assert!(report2.sessions_skipped >= 3, "2 transcripts + 1 instruction skipped");
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
