//! Persona pipeline end-to-end (doc 06 §6.10): readers → digest → facet trees →
//! compiled pack, driven by the OpenRouter reference provider pointed at a
//! wiremock server. Exercises pack structure, tier ordering, incremental
//! cursor resume after a partial run, budget cutoff, and the cost assertion.
//!
//! Requires features: `persona`, `providers-http`, `git-diff`.

use std::io::Write;
use std::path::Path;
use std::process::Command;

use serde_json::json;
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use tinycortex::memory::config::{MemoryConfig, SecretString};
use tinycortex::memory::persona::config::PersonaConfig;
use tinycortex::memory::persona::state::FileStateStore;
use tinycortex::memory::persona::{Pipeline, RunMode};
use tinycortex::memory::providers::{OpenRouterConfig, OpenRouterProvider};

/// A digest response the mock returns for every chat call (JSON-mode digests and
/// plain-text folds alike accept it).
const DIGEST_JSON: &str = r#"{"observations":[
    {"facet":"workflow","observation":"Commits small and often on feature branches","quote":"commit small","tier":"t2"},
    {"facet":"communication","observation":"Terse and directive","quote":"just do it","tier":"t2"},
    {"facet":"coding_style","observation":"Insists on regression tests","quote":"add a test","tier":"t1"}
]}"#;

fn chat_body() -> serde_json::Value {
    json!({
        "id": "gen-1",
        "choices": [{"index": 0, "message": {"role": "assistant", "content": DIGEST_JSON}}],
        "usage": {"prompt_tokens": 50, "completion_tokens": 30, "total_tokens": 80, "cost": 0.0001}
    })
}

async fn mock_openrouter() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(chat_body()))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{"index": 0, "embedding": [0.1, 0.2, 0.3]}],
            "usage": {"prompt_tokens": 3, "total_tokens": 3, "cost": 0.0}
        })))
        .mount(&server)
        .await;
    server
}

fn write(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut f = std::fs::File::create(path).unwrap();
    f.write_all(contents.as_bytes()).unwrap();
}

/// Build fixtures: two Claude Code transcripts, one Codex rollout, an
/// instruction file, and a small git repo. Returns the sources tempdir.
fn build_fixtures() -> TempDir {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    // Claude Code transcripts.
    for (i, name) in ["a.jsonl", "b.jsonl"].iter().enumerate() {
        let ts = format!("2026-07-0{}T10:00:00.000Z", i + 1);
        write(
            &root.join("claude/projects/-work-demo").join(name),
            &format!(
                r#"{{"type":"user","isSidechain":false,"cwd":"/work/demo","sessionId":"s{i}","timestamp":"{ts}","message":{{"role":"user","content":"implement the parser and commit small"}}}}
{{"type":"assistant","timestamp":"{ts}","message":{{"role":"assistant","content":[{{"type":"text","text":"I loaded the whole file."}}]}}}}
{{"type":"user","isSidechain":false,"sessionId":"s{i}","timestamp":"{ts}","message":{{"role":"user","content":"no, stream it instead"}}}}
"#
            ),
        );
    }

    // Codex rollout.
    write(
        &root.join("codex/sessions/2026/07/01/rollout-2026-07-01T10-00-00-x.jsonl"),
        r#"{"timestamp":"2026-07-01T10:00:00.000Z","type":"session_meta","payload":{"id":"cx1","cwd":"/work/demo"}}
{"timestamp":"2026-07-01T10:00:02.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"resolve this issue and open a PR"}]}}
"#,
    );

    // Instruction file + git repo under a project root.
    let repo = root.join("proj");
    write(
        &repo.join("CLAUDE.md"),
        "- Always branch before writing code.\n- Commit regularly with clear messages.\n",
    );
    init_git_repo(&repo);

    dir
}

/// Initialise a git repo with one author commit via the `git` CLI.
fn init_git_repo(dir: &Path) {
    let run = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "Me")
            .env("GIT_AUTHOR_EMAIL", "me@work.com")
            .env("GIT_COMMITTER_NAME", "Me")
            .env("GIT_COMMITTER_EMAIL", "me@work.com")
            .output()
            .unwrap();
    };
    run(&["init", "-q"]);
    std::fs::write(dir.join("lib.rs"), "fn parse() {}\n").unwrap();
    run(&["add", "."]);
    run(&["commit", "-q", "-m", "feat: add streaming parser"]);
}

fn persona_config(sources: &Path) -> PersonaConfig {
    let mut cfg = PersonaConfig::with_home(sources, "me@example.com");
    cfg.claude_code_root = Some(sources.join("claude/projects"));
    cfg.codex_root = Some(sources.join("codex/sessions"));
    cfg.project_roots = vec![sources.join("proj")];
    cfg.global_instruction_files = vec![];
    cfg.author_emails = vec!["me@work.com".into()];
    cfg
}

fn provider(server: &MockServer, persona: &PersonaConfig) -> OpenRouterProvider {
    OpenRouterProvider::new(OpenRouterConfig {
        base_url: server.uri(),
        api_key: SecretString::new("sk-test"),
        chat_model: persona.chat_model.clone(),
        run_cost_limit_usd: Some(persona.run_budget.max_cost_usd),
        run_call_limit: Some(persona.run_budget.max_llm_calls),
        ..Default::default()
    })
    .unwrap()
}

#[tokio::test]
async fn backfill_produces_structured_pack_within_budget() {
    let server = mock_openrouter().await;
    let sources = build_fixtures();
    let ws = TempDir::new().unwrap();
    let config = MemoryConfig::new(ws.path());
    let persona = persona_config(sources.path());
    let prov = provider(&server, &persona);
    let store = FileStateStore::open_in_workspace(ws.path()).unwrap();

    let pipeline = Pipeline {
        config: &config,
        persona: &persona,
        provider: &prov,
        summariser: &prov,
        store: &store,
    };
    let report = pipeline.run(RunMode::Backfill).await.unwrap();

    // Transcripts (2 Claude + 1 Codex) + git batch were digested.
    assert!(report.sessions_processed >= 3, "report: {report:?}");
    assert_eq!(report.directives_folded, 2, "two instruction rules folded");
    assert!(report.observations >= 3);

    // Pack structure: identity header, Directives section present, ordering.
    let pack = std::fs::read_to_string(report.pack_path.as_ref().unwrap()).unwrap();
    assert!(pack.starts_with("# Persona: me@example.com"));
    assert!(pack.contains("## Directives"));
    assert!(pack.contains("Always branch before writing code."));
    assert!(pack.contains("## Workflow"));
    let d = pack.find("## Directives").unwrap();
    let w = pack.find("## Workflow").unwrap();
    assert!(d < w, "directives must precede workflow");

    // Cost assertion: spend stayed under the configured run ceiling.
    let usage = prov.usage();
    assert!(usage.requests > 0);
    assert!(usage.cost_usd <= persona.run_budget.max_cost_usd);
    assert!((usage.cost_usd - usage.requests as f64 * 0.0001).abs() < 1e-9);
}

#[tokio::test]
async fn incremental_resume_after_partial_backfill() {
    let server = mock_openrouter().await;
    let sources = build_fixtures();
    let ws = TempDir::new().unwrap();
    let config = MemoryConfig::new(ws.path());
    let mut persona = persona_config(sources.path());
    // Partial: only one session may digest, forcing a checkpoint.
    persona.run_budget.max_sessions = 1;
    let prov = provider(&server, &persona);
    let store = FileStateStore::open_in_workspace(ws.path()).unwrap();

    let first = {
        let pipeline = Pipeline {
            config: &config,
            persona: &persona,
            provider: &prov,
            summariser: &prov,
            store: &store,
        };
        pipeline.run(RunMode::Backfill).await.unwrap()
    };
    assert_eq!(first.sessions_processed, 1);
    assert!(first.budget_hit);

    // Lift the cap and resume incrementally: the remaining sessions process,
    // the already-cursored ones are skipped.
    persona.run_budget.max_sessions = 100;
    let prov2 = provider(&server, &persona);
    let pipeline = Pipeline {
        config: &config,
        persona: &persona,
        provider: &prov2,
        summariser: &prov2,
        store: &store,
    };
    let second = pipeline.run(RunMode::Incremental).await.unwrap();
    assert!(second.sessions_processed >= 1, "remaining sessions resumed");
    assert!(
        second.sessions_skipped >= 1,
        "the checkpointed file was skipped"
    );

    // Final pack still well-formed.
    let pack = std::fs::read_to_string(second.pack_path.as_ref().unwrap()).unwrap();
    assert!(pack.contains("# Persona: me@example.com"));
}

#[tokio::test]
async fn offline_compile_only_needs_no_provider() {
    // Prove the compile subcommand re-assembles from trees without any LLM.
    let server = mock_openrouter().await;
    let sources = build_fixtures();
    let ws = TempDir::new().unwrap();
    let config = MemoryConfig::new(ws.path());
    let persona = persona_config(sources.path());
    let prov = provider(&server, &persona);
    let store = FileStateStore::open_in_workspace(ws.path()).unwrap();

    {
        let pipeline = Pipeline {
            config: &config,
            persona: &persona,
            provider: &prov,
            summariser: &prov,
            store: &store,
        };
        pipeline.run(RunMode::Backfill).await.unwrap();
    }

    // Re-compile using a deterministic summariser and a never-called provider.
    let concat = tinycortex::memory::tree::summarise::ConcatSummariser::new();
    let pipeline = Pipeline {
        config: &config,
        persona: &persona,
        provider: &prov, // not invoked by compile_only
        summariser: &concat,
        store: &store,
    };
    let path = pipeline.compile_only().unwrap();
    let pack = std::fs::read_to_string(path).unwrap();
    assert!(pack.contains("## Directives"));
}
