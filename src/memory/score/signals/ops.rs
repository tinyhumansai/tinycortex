//! Cross-signal helpers: signal computation entry point and the two
//! weighted-combine variants (full and cheap-only) used by `score_chunk`.

use super::{interaction, metadata_weight, source_weight, token_count, unique_words};
use super::{ScoreSignals, SignalWeights};
use crate::memory::chunks::Metadata;
use crate::memory::score::extract::ExtractedEntities;

/// Compute all signals for a chunk.
///
/// `llm_importance` is sourced from `ex.llm_importance` (defaults to `0.0`
/// when the extractor didn't produce one — equivalent to "no LLM signal").
pub fn compute(
    meta: &Metadata,
    content: &str,
    token_count: u32,
    ex: &ExtractedEntities,
) -> ScoreSignals {
    ScoreSignals {
        token_count: token_count::score(token_count),
        unique_words: unique_words::score(content),
        metadata_weight: metadata_weight::score(meta),
        source_weight: source_weight::score(meta),
        interaction: interaction::score(meta),
        entity_density: entity_density_score(token_count, ex),
        llm_importance: ex.llm_importance.unwrap_or(0.0).clamp(0.0, 1.0),
    }
}

/// Entity-density signal: entities per token, capped.
///
/// More distinct entities per unit of content → more substantive. Calibrated
/// so ~1 entity per 100 tokens maxes out the signal.
pub fn entity_density_score(token_count: u32, ex: &ExtractedEntities) -> f32 {
    let unique = ex.unique_entity_count() as f32;
    if token_count == 0 {
        return 0.0;
    }
    let per_token = unique / token_count as f32;
    // cap at 0.01 entities/token = 1 entity per 100 tokens
    (per_token / 0.01).min(1.0)
}

/// Weighted sum of signals, normalised to `[0.0, 1.0]`.
///
/// When `w.llm_importance == 0.0` (the default) the LLM signal contributes
/// nothing to either the numerator or the denominator — output is identical
/// to pre-LLM Phase 2.
pub fn combine(signals: &ScoreSignals, w: &SignalWeights) -> f32 {
    let total_weight = w.token_count
        + w.unique_words
        + w.metadata_weight
        + w.source_weight
        + w.interaction
        + w.entity_density
        + w.llm_importance;
    if total_weight <= 0.0 {
        return 0.0;
    }
    let weighted = signals.token_count * w.token_count
        + signals.unique_words * w.unique_words
        + signals.metadata_weight * w.metadata_weight
        + signals.source_weight * w.source_weight
        + signals.interaction * w.interaction
        + signals.entity_density * w.entity_density
        + signals.llm_importance * w.llm_importance;
    (weighted / total_weight).clamp(0.0, 1.0)
}

/// Weighted sum **excluding the `llm_importance` signal**.
///
/// Used by the short-circuit logic in `score_chunk`: if the deterministic
/// (cheap-signals-only) total is already firmly above or below the
/// admission band, we skip the LLM call entirely. The LLM signal only
/// participates in the *final* `combine` once it's been computed.
pub fn combine_cheap_only(signals: &ScoreSignals, w: &SignalWeights) -> f32 {
    let total_weight = w.token_count
        + w.unique_words
        + w.metadata_weight
        + w.source_weight
        + w.interaction
        + w.entity_density;
    if total_weight <= 0.0 {
        return 0.0;
    }
    let weighted = signals.token_count * w.token_count
        + signals.unique_words * w.unique_words
        + signals.metadata_weight * w.metadata_weight
        + signals.source_weight * w.source_weight
        + signals.interaction * w.interaction
        + signals.entity_density * w.entity_density;
    (weighted / total_weight).clamp(0.0, 1.0)
}

#[cfg(test)]
#[path = "ops_tests.rs"]
mod tests;
