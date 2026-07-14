//! Persona decision agent (doc 06 follow-on): answer hard coding decisions
//! *as this person would*, grounded in their distilled persona memory layer.
//!
//! Two explicit stages, matching the design brief:
//!
//! 1. **Algorithmic retrieval (no LLM).** [`PersonaRetriever`] loads every
//!    persona observation leaf the pipeline persisted and ranks them against a
//!    query with a deterministic, network-free BM25 scorer weighted by evidence
//!    tier. This stage never calls a model.
//! 2. **LLM final pass (intelligence / filter).** A [`tinyagents`] agent harness,
//!    backed by the OpenRouter reference model (DeepSeek v4 Flash by default),
//!    is given three read-only tools over that retriever. It decides what to look
//!    up, filters the candidates, resolves conflicts by tier/recency, and writes
//!    a decision in the person's own voice — citing the evidence it used.
//!
//! The retriever is the agent's *only* window onto the person: the model is
//! instructed to grow its answer strictly from retrieved evidence, never from
//! generic best practice.
//!
//! ## Usage
//!
//! ```sh
//! # after a `persona_harness backfill` has populated the workspace:
//! cargo run --example persona_agent --features persona -- "Should I add a new
//!   crate dependency or vendor a small helper myself?"
//! ```
//!
//! - `TINYCORTEX_WORKSPACE` — persona workspace (default `./persona-workspace`).
//! - `OPENROUTER_API_KEY`   — required (loaded from `.env` via dotenvy).
//! - `TINYCORTEX_LLM_MODEL` — chat model id (default `deepseek/deepseek-v4-flash`).

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use tinyagents::harness::message::Message;
use tinyagents::harness::providers::openai::OpenAiModel;
use tinyagents::harness::runtime::AgentHarness;
use tinyagents::harness::tool::{Tool, ToolCall, ToolPolicy, ToolResult, ToolSchema};

use tinycortex::memory::config::MemoryConfig;
use tinycortex::memory::persona::compile::read_directives;
use tinycortex::memory::persona::retrieve::{PersonaHit, PersonaRetriever};
use tinycortex::memory::persona::types::PersonaFacet;

/// Default number of observations the retriever surfaces per query.
const DEFAULT_K: usize = 8;

/// Shared agent state: the loaded memory layer and the person's verbatim rules.
struct PersonaState {
    retriever: PersonaRetriever,
    directives: Vec<String>,
    identity: String,
}

// ─────────────────────────── Tools (over the memory layer) ───────────────────

/// Stage-1 retrieval exposed as a tool: deterministic BM25, no model call.
struct SearchPersonaTool;

#[async_trait]
impl Tool<PersonaState> for SearchPersonaTool {
    fn name(&self) -> &str {
        "search_persona"
    }
    fn description(&self) -> &str {
        "Search the person's distilled coding persona for observations relevant \
         to a query. Purely algorithmic BM25 ranking weighted by evidence tier — \
         no LLM. Optionally restrict to one facet."
    }
    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "search_persona",
            "Retrieve the most relevant persona observations for a query.",
            json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Natural-language description of what you need to know about how this person works."
                    },
                    "facet": {
                        "type": "string",
                        "description": "Optional facet filter.",
                        "enum": [
                            "communication", "coding_style", "stack", "workflow",
                            "environment", "directives", "anti_preferences"
                        ]
                    },
                    "k": {
                        "type": "integer",
                        "description": "Max observations to return (default 8).",
                        "minimum": 1,
                        "maximum": 20
                    }
                },
                "required": ["query"]
            }),
        )
    }
    fn policy(&self) -> ToolPolicy {
        ToolPolicy::read_only()
    }
    async fn call(&self, state: &PersonaState, call: ToolCall) -> tinyagents::Result<ToolResult> {
        let query = call
            .arguments
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let facet = call
            .arguments
            .get("facet")
            .and_then(|v| v.as_str())
            .and_then(PersonaFacet::parse_loose);
        let k = call
            .arguments
            .get("k")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(DEFAULT_K)
            .clamp(1, 20);

        let hits = state.retriever.search(query, facet, k);
        Ok(ToolResult::text(
            call.id,
            "search_persona",
            format_hits(&hits),
        ))
    }
}

/// Return the person's explicit, verbatim standing rules (mostly T0 directives).
struct ListDirectivesTool;

#[async_trait]
impl Tool<PersonaState> for ListDirectivesTool {
    fn name(&self) -> &str {
        "list_directives"
    }
    fn description(&self) -> &str {
        "List the person's explicit, written-down standing rules for their coding \
         agents (highest-authority evidence). Consult these before any decision."
    }
    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "list_directives",
            "List the person's verbatim standing rules.",
            json!({ "type": "object", "properties": {} }),
        )
    }
    fn policy(&self) -> ToolPolicy {
        ToolPolicy::read_only()
    }
    async fn call(&self, state: &PersonaState, call: ToolCall) -> tinyagents::Result<ToolResult> {
        let body = if state.directives.is_empty() {
            "No explicit directives recorded.".to_string()
        } else {
            state
                .directives
                .iter()
                .map(|d| format!("- {d}"))
                .collect::<Vec<_>>()
                .join("\n")
        };
        Ok(ToolResult::text(call.id, "list_directives", body))
    }
}

/// Summarise the coverage of the loaded memory layer (facet counts).
struct PersonaOverviewTool;

#[async_trait]
impl Tool<PersonaState> for PersonaOverviewTool {
    fn name(&self) -> &str {
        "persona_overview"
    }
    fn description(&self) -> &str {
        "Report which facets of the person's persona have evidence and how much, \
         so you know where the memory layer is strong or thin."
    }
    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "persona_overview",
            "Report per-facet observation coverage.",
            json!({ "type": "object", "properties": {} }),
        )
    }
    fn policy(&self) -> ToolPolicy {
        ToolPolicy::read_only()
    }
    async fn call(&self, state: &PersonaState, call: ToolCall) -> tinyagents::Result<ToolResult> {
        Ok(ToolResult::text(
            call.id,
            "persona_overview",
            overview(state),
        ))
    }
}

// ─────────────────────────────── Formatting ─────────────────────────────────

fn format_hits(hits: &[PersonaHit]) -> String {
    if hits.is_empty() {
        return "No matching persona evidence found for that query.".to_string();
    }
    hits.iter()
        .map(|h| {
            let quote = h
                .quote
                .as_deref()
                .map(|q| format!(" — quote: \"{q}\""))
                .unwrap_or_default();
            format!(
                "[{facet} | {tier} | score {score:.2}] {text}{quote}",
                facet = h.facet.as_str(),
                tier = h.tier.as_str(),
                score = h.score,
                text = h.text,
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn overview(state: &PersonaState) -> String {
    let counts = state.retriever.facet_counts();
    let mut lines = vec![format!(
        "Identity: {}\nTotal observations: {}\nDirectives: {}",
        state.identity,
        state.retriever.len(),
        state.directives.len(),
    )];
    for facet in PersonaFacet::ALL {
        lines.push(format!(
            "- {}: {} observations",
            facet.heading(),
            counts.get(&facet).copied().unwrap_or(0)
        ));
    }
    lines.join("\n")
}

const SYSTEM_PROMPT: &str = "\
You are the coding alter-ego of a specific developer. You answer hard \
engineering decisions, design questions, and \"what should I do here?\" blocks \
*as they would* — reflecting their real, evidenced habits, stack, workflow, and \
pet peeves — not generic best practice.

Your ONLY window onto this person is the memory tools. Ground every claim in \
retrieved evidence:
- Start by calling `persona_overview` (once) and `list_directives` to anchor on \
  their explicit rules.
- Then call `search_persona` one or more times with focused queries (use the \
  `facet` filter to target coding_style, stack, workflow, etc.). Retrieval is \
  algorithmic and free — search liberally and from multiple angles before you \
  answer.
- Evidence carries a confidence tier: t0 = a rule they wrote down, t1 = an \
  in-transcript correction, t2 = habitual phrasing/commits, t3 = inferred \
  outcome. Prefer higher tiers; when evidence conflicts, the higher tier and \
  more recent observation wins. t3 alone only corroborates — never decide on it.

Then answer. Requirements for the answer:
1. Give a decisive recommendation in their voice, not a menu of options.
2. Justify it by citing the specific observations/directives you relied on \
   (paraphrase + tier), so the reasoning is auditable.
3. If the memory layer has little or no relevant evidence, say so plainly and \
   flag that you are extrapolating — do not fabricate a preference.
Keep it tight and concrete.";

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();

    let question = {
        let joined = std::env::args().skip(1).collect::<Vec<_>>().join(" ");
        if joined.trim().is_empty() {
            "I need to add retry/backoff to an HTTP client in one of my Rust \
             services. Should I pull in a dependency for it, hand-roll it, or \
             something else? Decide the way I would."
                .to_string()
        } else {
            joined
        }
    };

    // ── Load the memory layer ────────────────────────────────────────────────
    let workspace = std::env::var("TINYCORTEX_WORKSPACE")
        .unwrap_or_else(|_| "./persona-workspace".to_string());
    let config = MemoryConfig::new(&workspace);
    let retriever = PersonaRetriever::load(&config)?;
    let directives = read_directives(&config);
    let identity =
        std::env::var("PERSONA_IDENTITY").unwrap_or_else(|_| "this developer".to_string());

    if retriever.is_empty() {
        anyhow::bail!(
            "persona memory layer at {workspace} is empty — run the persona \
             backfill first (examples/persona_harness backfill)."
        );
    }

    let state = PersonaState {
        retriever,
        directives,
        identity,
    };

    println!("persona decision agent");
    println!("  workspace: {workspace}");
    println!("  {}", overview(&state).replace('\n', "\n  "));
    println!("\n════════════════════════ QUESTION ════════════════════════");
    println!("{question}");

    // ── Stage 1: algorithmic retrieval (no LLM) ──────────────────────────────
    println!("\n──────── Stage 1 · algorithmic retrieval (no LLM) ─────────");
    let seed_hits = state.retriever.search(&question, None, DEFAULT_K);
    println!("{}", format_hits(&seed_hits));

    // ── Stage 2: LLM final pass over the memory tools ────────────────────────
    let api_key = std::env::var("OPENROUTER_API_KEY")
        .map_err(|_| anyhow::anyhow!("OPENROUTER_API_KEY not set (needed for the LLM pass)"))?;
    let model_id = std::env::var("TINYCORTEX_LLM_MODEL")
        .unwrap_or_else(|_| "deepseek/deepseek-v4-flash".to_string());
    let model = OpenAiModel::openrouter(api_key).with_model(&model_id);

    let mut harness: AgentHarness<PersonaState> = AgentHarness::new();
    harness
        .register_model("openrouter", Arc::new(model))
        .set_default_model("openrouter")
        .register_tool(Arc::new(SearchPersonaTool))
        .register_tool(Arc::new(ListDirectivesTool))
        .register_tool(Arc::new(PersonaOverviewTool));

    println!("\n──────── Stage 2 · LLM synthesis pass ({model_id}) ────────");
    let started = std::time::Instant::now();
    let run = harness
        .invoke_default(
            &state,
            vec![
                Message::system(SYSTEM_PROMPT),
                Message::user(&question),
            ],
        )
        .await?;

    println!("\n════════════════════════ DECISION ════════════════════════");
    println!("{}", run.text().unwrap_or_default());
    println!("\n───────────────────────── telemetry ──────────────────────");
    println!(
        "  model calls: {}  tool calls: {}  steps: {}",
        run.model_calls, run.tool_calls, run.steps
    );
    println!(
        "  tokens: {} in + {} out = {} total",
        run.usage.usage.input_tokens, run.usage.usage.output_tokens, run.usage.usage.total_tokens
    );
    println!("  wall-clock: {:.1}s", started.elapsed().as_secs_f64());

    Ok(())
}
