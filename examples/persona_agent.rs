//! Persona decision assistant: answer hard coding decisions as this person
//! would, grounded in their distilled persona memory layer.
//!
//! The workflow has two explicit stages:
//!
//! 1. [`PersonaRetriever`] ranks persisted observations with deterministic,
//!    network-free BM25 retrieval weighted by evidence tier.
//! 2. A provider-neutral [`tinyinference`] model receives only those ranked
//!    observations and the person's explicit directives, then writes one
//!    evidence-cited decision.
//!
//! TinyCortex owns retrieval and memory. TinyInference owns the model API and
//! provider transport. No agent loop, tool registry, or session runtime is
//! needed for this bounded synthesis pass.
//!
//! ## Usage
//!
//! ```sh
//! cargo run --example persona_agent --features persona -- "Should I add a new
//! crate dependency or vendor a small helper myself?"
//! ```
//!
//! - `TINYCORTEX_WORKSPACE` — persona workspace (default `./persona-workspace`).
//! - `OPENROUTER_API_KEY` — required (loaded from `.env` via dotenvy).
//! - `TINYCORTEX_LLM_MODEL` — model id (default `deepseek/deepseek-v4-flash`).

use tinyinference::message::Message;
use tinyinference::model::{ChatModel, ModelRequest};
use tinyinference::providers::openai::OpenAiModel;

use tinycortex::memory::config::MemoryConfig;
use tinycortex::memory::persona::compile::read_directives;
use tinycortex::memory::persona::retrieve::{PersonaHit, PersonaRetriever};
use tinycortex::memory::persona::types::PersonaFacet;

/// Default number of observations included in the synthesis prompt.
const DEFAULT_K: usize = 12;

fn format_hits(hits: &[PersonaHit]) -> String {
    if hits.is_empty() {
        return "No matching persona evidence found for that query.".to_string();
    }
    hits.iter()
        .map(|hit| {
            let quote = hit
                .quote
                .as_deref()
                .map(|quote| format!(" — quote: \"{quote}\""))
                .unwrap_or_default();
            format!(
                "[{facet} | {tier} | score {score:.2}] {text}{quote}",
                facet = hit.facet.as_str(),
                tier = hit.tier.as_str(),
                score = hit.score,
                text = hit.text,
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn overview(retriever: &PersonaRetriever, directives: &[String], identity: &str) -> String {
    let counts = retriever.facet_counts();
    let mut lines = vec![format!(
        "Identity: {identity}\nTotal observations: {}\nDirectives: {}",
        retriever.len(),
        directives.len(),
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
You are the coding alter-ego of a specific developer. Give one decisive, \
concrete recommendation in their voice, grounded only in the supplied persona \
evidence and explicit directives rather than generic best practice. Cite the \
specific observations and evidence tiers you rely on. Prefer t0 over t1, t1 \
over t2, and use t3 only as corroboration. If evidence is thin or conflicting, \
say so plainly instead of inventing a preference.";

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();

    let question = {
        let joined = std::env::args().skip(1).collect::<Vec<_>>().join(" ");
        if joined.trim().is_empty() {
            "I need to add retry/backoff to an HTTP client in one of my Rust \
             services. Should I pull in a dependency, hand-roll it, or do \
             something else? Decide the way I would."
                .to_string()
        } else {
            joined
        }
    };

    let workspace =
        std::env::var("TINYCORTEX_WORKSPACE").unwrap_or_else(|_| "./persona-workspace".to_string());
    let config = MemoryConfig::new(&workspace);
    let retriever = PersonaRetriever::load(&config)?;
    let directives = read_directives(&config);
    let identity =
        std::env::var("PERSONA_IDENTITY").unwrap_or_else(|_| "this developer".to_string());

    if retriever.is_empty() {
        anyhow::bail!(
            "persona memory layer at {workspace} is empty — run the persona \
             backfill first (examples/persona_harness backfill)"
        );
    }

    println!("persona decision assistant");
    println!("  workspace: {workspace}");
    println!(
        "  {}",
        overview(&retriever, &directives, &identity).replace('\n', "\n  ")
    );
    println!("\n════════════════════════ QUESTION ════════════════════════");
    println!("{question}");

    let hits = retriever.search(&question, None, DEFAULT_K);
    let evidence = format_hits(&hits);
    println!("\n──────── Stage 1 · algorithmic retrieval (no LLM) ─────────");
    println!("{evidence}");

    let directive_text = if directives.is_empty() {
        "No explicit directives recorded.".to_string()
    } else {
        directives
            .iter()
            .map(|directive| format!("- {directive}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let prompt = format!(
        "Identity and coverage:\n{}\n\nExplicit directives:\n{}\n\nQuestion:\n{}\n\nRanked persona evidence:\n{}",
        overview(&retriever, &directives, &identity),
        directive_text,
        question,
        evidence,
    );

    let api_key = std::env::var("OPENROUTER_API_KEY")
        .map_err(|_| anyhow::anyhow!("OPENROUTER_API_KEY not set (needed for the LLM pass)"))?;
    let model_id = std::env::var("TINYCORTEX_LLM_MODEL")
        .unwrap_or_else(|_| "deepseek/deepseek-v4-flash".to_string());
    let model = OpenAiModel::openrouter(api_key).with_model(&model_id);
    let request = ModelRequest::new(vec![Message::system(SYSTEM_PROMPT), Message::user(prompt)])
        .with_temperature(0.2);

    println!("\n──────── Stage 2 · LLM synthesis pass ({model_id}) ────────");
    let started = std::time::Instant::now();
    let response = model.invoke(&(), request).await?;

    println!("\n════════════════════════ DECISION ════════════════════════");
    println!("{}", response.text());
    println!("\n───────────────────────── telemetry ──────────────────────");
    if let Some(usage) = response.usage {
        println!(
            "  tokens: {} in + {} out = {} total",
            usage.input_tokens, usage.output_tokens, usage.total_tokens
        );
    }
    println!("  model calls: 1");
    println!("  wall-clock: {:.1}s", started.elapsed().as_secs_f64());

    Ok(())
}
