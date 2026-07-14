//! Persona distillation harness (doc 06 §6.8).
//!
//! Runs the persona pipeline end-to-end over this machine's local coding-agent
//! history, agent instruction files, and git commit history, distilling a
//! `persona/PERSONA.md` context pack via the OpenRouter reference provider
//! (DeepSeek v4 Flash by default for chat, an OpenAI-compatible embedding model
//! for vectors).
//!
//! ## Usage
//!
//! ```sh
//! cp .env.example .env   # then set OPENROUTER_API_KEY
//! cargo run --example persona_harness \
//!   --features persona,providers-http,git-diff -- [backfill|incremental|compile|status]
//! ```
//!
//! - `backfill`    — walk everything oldest-first, distil, write the pack.
//! - `incremental` — cursor-forward only: re-process changed files/repos.
//! - `compile`     — re-assemble the pack from existing facet trees (no LLM).
//! - `status`      — print roots, cursors, and the last pack path.
//!
//! ## Environment
//! - `OPENROUTER_API_KEY`        (required except for `compile`/`status`).
//! - `TINYCORTEX_WORKSPACE`      workspace dir (default `./persona-workspace`).
//! - `PERSONA_IDENTITY`          pack identity line (default: `$USER`).
//! - `PERSONA_AUTHOR_EMAILS`     comma-separated git author emails.
//! - `PERSONA_MAX_SESSIONS`      cap sessions digested this run.
//! - `PERSONA_MAX_COST_USD`      hard per-run provider spend ceiling.
//! - `TINYCORTEX_LLM_MODEL`      chat/digest model id.
//! - `TINYCORTEX_EMBED_MODEL`    embedding model id.
//! - `PERSONA_CLAUDE_ROOT` / `PERSONA_CODEX_ROOT` / `PERSONA_PROJECT_ROOTS`
//!   override the default source roots.

use std::path::PathBuf;
use std::sync::Arc;

use tinycortex::memory::config::{MemoryConfig, SecretString};
use tinycortex::memory::persona::config::PersonaConfig;
use tinycortex::memory::persona::state::FileStateStore;
use tinycortex::memory::persona::{Pipeline, RunMode};
use tinycortex::memory::providers::{OpenRouterConfig, OpenRouterProvider};

fn env_path(key: &str) -> Option<PathBuf> {
    std::env::var(key).ok().map(PathBuf::from)
}

fn home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

fn build_persona_config() -> PersonaConfig {
    let identity = std::env::var("PERSONA_IDENTITY")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "unknown".to_string());
    let mut cfg = PersonaConfig::with_home(&home(), identity);

    if let Some(p) = env_path("PERSONA_CLAUDE_ROOT") {
        cfg.claude_code_root = Some(p);
    }
    if let Some(p) = env_path("PERSONA_CODEX_ROOT") {
        cfg.codex_root = Some(p);
    }
    if let Ok(roots) = std::env::var("PERSONA_PROJECT_ROOTS") {
        cfg.project_roots = roots.split(',').map(|s| PathBuf::from(s.trim())).collect();
    }
    if let Ok(emails) = std::env::var("PERSONA_AUTHOR_EMAILS") {
        cfg.author_emails = emails
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
    }
    if let Ok(model) = std::env::var("TINYCORTEX_LLM_MODEL") {
        cfg.chat_model = model;
    }
    if let Ok(model) = std::env::var("TINYCORTEX_EMBED_MODEL") {
        cfg.embed_model = model;
    }
    if let Ok(n) = std::env::var("PERSONA_MAX_SESSIONS") {
        if let Ok(n) = n.parse() {
            cfg.run_budget.max_sessions = n;
        }
    }
    if let Ok(c) = std::env::var("PERSONA_MAX_COST_USD") {
        if let Ok(c) = c.parse() {
            cfg.run_budget.max_cost_usd = c;
        }
    }
    cfg
}

fn build_provider(persona: &PersonaConfig) -> anyhow::Result<Arc<OpenRouterProvider>> {
    let key = std::env::var("OPENROUTER_API_KEY").map_err(|_| {
        anyhow::anyhow!("OPENROUTER_API_KEY not set (needed for backfill/incremental)")
    })?;
    let provider = OpenRouterProvider::new(OpenRouterConfig {
        api_key: SecretString::new(key),
        chat_model: persona.chat_model.clone(),
        embed_model: persona.embed_model.clone(),
        run_cost_limit_usd: Some(persona.run_budget.max_cost_usd),
        run_call_limit: Some(persona.run_budget.max_llm_calls),
        ..Default::default()
    })?;
    Ok(Arc::new(provider))
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    let mode = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "status".to_string());

    let workspace =
        env_path("TINYCORTEX_WORKSPACE").unwrap_or_else(|| PathBuf::from("./persona-workspace"));
    std::fs::create_dir_all(&workspace)?;
    let config = MemoryConfig::new(&workspace);
    let persona = build_persona_config();
    let store = FileStateStore::open_in_workspace(&workspace)?;

    println!("persona harness: mode={mode}");
    println!("  workspace: {}", workspace.display());
    println!("  identity:  {}", persona.identity);
    println!(
        "  chat model: {}  embed model: {}",
        persona.chat_model, persona.embed_model
    );
    println!(
        "  roots: claude={:?} codex={:?} projects={:?}",
        persona.claude_code_root, persona.codex_root, persona.project_roots
    );

    match mode.as_str() {
        "backfill" | "incremental" => {
            let provider = build_provider(&persona)?;
            let run_mode = if mode == "backfill" {
                RunMode::Backfill
            } else {
                RunMode::Incremental
            };
            let pipeline = Pipeline {
                config: &config,
                persona: &persona,
                provider: provider.as_ref(),
                summariser: provider.as_ref(),
                store: &store,
            };
            let started = std::time::Instant::now();
            let report = pipeline.run(run_mode).await?;
            let usage = provider.usage();
            println!("\n── run report ──");
            println!("{}", serde_json::to_string_pretty(&report)?);
            println!(
                "  provider: {} requests, {} prompt + {} completion tokens, ${:.4}",
                usage.requests, usage.prompt_tokens, usage.completion_tokens, usage.cost_usd
            );
            println!("  wall-clock: {:.1}s", started.elapsed().as_secs_f64());
            if let Some(path) = &report.pack_path {
                println!("  pack: {path}");
            }
        }
        "compile" => {
            let provider = build_provider(&persona).ok();
            // compile_only needs a summariser only to (re)open flavoured trees;
            // it performs no folds, so any provider — or none — works. Fall back
            // to a deterministic summariser when the key is absent.
            let concat = tinycortex::memory::tree::summarise::ConcatSummariser::new();
            let summariser: &dyn tinycortex::memory::tree::Summariser = match &provider {
                Some(p) => p.as_ref(),
                None => &concat,
            };
            // A dummy chat provider is never called by compile_only.
            let pipeline = Pipeline {
                config: &config,
                persona: &persona,
                provider: &NullChat,
                summariser,
                store: &store,
            };
            let path = pipeline.compile_only()?;
            println!("  recompiled pack: {}", path.display());
        }
        "status" => {
            let pack = tinycortex::memory::persona::compile::pack_path(&config);
            println!("\n── status ──");
            println!("  pack exists: {}  ({})", pack.exists(), pack.display());
            let state_file = workspace.join("persona/sync-state.json");
            if state_file.exists() {
                let bytes = std::fs::read(&state_file)?;
                let map: serde_json::Value = serde_json::from_slice(&bytes)?;
                let n = map.as_object().map(|o| o.len()).unwrap_or(0);
                println!("  tracked cursors: {n}");
            } else {
                println!("  tracked cursors: 0 (no prior run)");
            }
        }
        other => {
            eprintln!("unknown mode '{other}' (expected backfill|incremental|compile|status)");
            std::process::exit(2);
        }
    }
    Ok(())
}

/// A chat provider that is never invoked (used by `compile`, which does no map).
struct NullChat;

#[async_trait::async_trait]
impl tinycortex::memory::score::extract::ChatProvider for NullChat {
    fn name(&self) -> &str {
        "null"
    }
    async fn chat_for_json(
        &self,
        _p: &tinycortex::memory::score::extract::ChatPrompt,
    ) -> anyhow::Result<String> {
        anyhow::bail!("NullChat must not be called")
    }
}
