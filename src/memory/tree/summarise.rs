//! Memory-tree summariser: fold N inputs into one parent summary.
//!
//! OpenHuman's `memory_tree::summarise` made a real chat-provider call. Here
//! the LLM is abstracted behind the [`Summariser`] trait so the engine never
//! depends on a network backend. The default [`ConcatSummariser`] is fully
//! deterministic (concatenate-with-provenance + truncate-to-budget), which is
//! also the [`fallback_summary`] a host wires its own LLM-backed [`Summariser`]
//! over and falls back to when the model errors or returns blank output.

use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::memory::chunks::approx_token_count;
use crate::memory::tree::store::TreeKind;

/// One contribution being folded — a raw leaf at L0→L1, or a lower-level
/// summary at L_n→L_{n+1}.
#[derive(Clone, Debug)]
pub struct SummaryInput {
    pub id: String,
    pub content: String,
    pub token_count: u32,
    pub entities: Vec<String>,
    pub topics: Vec<String>,
    pub time_range_start: DateTime<Utc>,
    pub time_range_end: DateTime<Utc>,
    pub score: f32,
}

/// Per-seal context — identifies which tree/level is being sealed.
#[derive(Clone, Debug)]
pub struct SummaryContext<'a> {
    pub tree_id: &'a str,
    pub tree_kind: TreeKind,
    pub target_level: u32,
    pub token_budget: u32,
}

/// Output of a summarise call.
#[derive(Clone, Debug, Default)]
pub struct SummaryOutput {
    pub content: String,
    pub token_count: u32,
    /// Always emitted empty by the built-in summarisers; canonical entity ids
    /// are populated separately by the seal-time label strategy.
    pub entities: Vec<String>,
    pub topics: Vec<String>,
}

/// Backend that folds inputs into one summary. Abstracted so the crate never
/// calls a real LLM; the default is the deterministic [`ConcatSummariser`].
#[async_trait]
pub trait Summariser: Send + Sync {
    /// Stable short name for diagnostics.
    fn name(&self) -> &str {
        "summariser"
    }

    /// Fold `inputs` into a single summary, clamped to `ctx.token_budget`.
    /// Returns `Err` on backend failure; seal cascades fall back to
    /// [`fallback_summary`] in that case.
    async fn summarise(
        &self,
        inputs: &[SummaryInput],
        ctx: &SummaryContext<'_>,
    ) -> Result<SummaryOutput>;
}

/// Deterministic, dependency-free summariser: concatenate inputs (priority-first
/// by score) with a provenance prefix and truncate to budget. Identical output
/// to [`fallback_summary`].
#[derive(Clone, Copy, Debug, Default)]
pub struct ConcatSummariser;

impl ConcatSummariser {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Summariser for ConcatSummariser {
    fn name(&self) -> &str {
        "concat"
    }

    async fn summarise(
        &self,
        inputs: &[SummaryInput],
        ctx: &SummaryContext<'_>,
    ) -> Result<SummaryOutput> {
        Ok(fallback_summary(inputs, ctx.token_budget))
    }
}

/// Deterministic concat-and-truncate fold. Each non-blank input is joined with
/// a `"— "` provenance prefix; the result is clamped to `budget` tokens.
pub fn fallback_summary(inputs: &[SummaryInput], budget: u32) -> SummaryOutput {
    const PROVENANCE_PREFIX: &str = "— ";
    // Priority-first by score so the most important material is least likely
    // to be truncated under budget pressure; `sort_by` is stable so equal-score
    // inputs keep chronological order.
    let mut order: Vec<&SummaryInput> = inputs.iter().collect();
    order.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut parts: Vec<String> = Vec::with_capacity(order.len());
    for inp in order {
        let trimmed = inp.content.trim();
        if trimmed.is_empty() {
            continue;
        }
        parts.push(format!("{PROVENANCE_PREFIX}{trimmed}"));
    }
    let joined = parts.join("\n\n");
    let (content, token_count) = clamp_to_budget(&joined, budget);
    SummaryOutput {
        content,
        token_count,
        entities: Vec::new(),
        topics: Vec::new(),
    }
}

/// Truncate `text` to at most `budget` approximate tokens. Returns the
/// (possibly clamped) text and its token estimate.
pub fn clamp_to_budget(text: &str, budget: u32) -> (String, u32) {
    let initial = approx_token_count(text);
    if initial <= budget {
        return (text.to_string(), initial);
    }
    let char_ceiling = (budget as usize).saturating_mul(4);
    let truncated: String = text.chars().take(char_ceiling).collect();
    let tokens = approx_token_count(&truncated);
    (truncated, tokens)
}

#[cfg(test)]
#[path = "summarise_tests.rs"]
mod tests;
