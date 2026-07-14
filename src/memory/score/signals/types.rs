//! Strongly-typed bag of per-signal scores plus the weights used to combine
//! them. Persisted alongside the total in `mem_tree_score` so a chunk's
//! admit/drop decision is auditable after the fact.

use serde::{Deserialize, Serialize};

/// Per-signal score breakdown for one chunk. Persisted alongside the total
/// for diagnostics.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ScoreSignals {
    /// Length signal derived from the chunk's token count.
    pub token_count: f32,
    /// Lexical-diversity signal derived from the count of distinct words.
    pub unique_words: f32,
    /// Contribution from structural/front-matter metadata on the source.
    pub metadata_weight: f32,
    /// Contribution from the source's provenance/authority.
    pub source_weight: f32,
    /// Direct-engagement signal from user interaction with the chunk.
    pub interaction: f32,
    /// Signal proportional to the density of extracted entities in the chunk.
    pub entity_density: f32,
    /// LLM-derived importance rating in `[0.0, 1.0]`. `0.0` when no LLM
    /// signal is available — combined with `SignalWeights::llm_importance = 0.0`
    /// (the default) this produces a no-op contribution to the total, keeping
    /// behaviour identical to pre-LLM Phase 2.
    ///
    /// Note: this signal is an in-memory admission input only. The persisted
    /// `mem_tree_score` schema does not carry an `llm_importance` column, so it
    /// reads back as `0.0` from the store — the admission `total` it influenced
    /// is what the row records.
    #[serde(default)]
    pub llm_importance: f32,
}

/// Default weights applied to each signal in `combine`.
///
/// `llm_importance` defaults to `0.0` (disabled). Callers who configure an
/// LLM extractor should bump it (typical: 2.0 — comparable to the
/// metadata/source weights, well below the interaction-direct signal).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct SignalWeights {
    /// Multiplier for [`ScoreSignals::token_count`]. Default `1.0`.
    pub token_count: f32,
    /// Multiplier for [`ScoreSignals::unique_words`]. Default `1.0`.
    pub unique_words: f32,
    /// Multiplier for [`ScoreSignals::metadata_weight`]. Default `1.5`.
    pub metadata_weight: f32,
    /// Multiplier for [`ScoreSignals::source_weight`]. Default `1.5`.
    pub source_weight: f32,
    /// Multiplier for [`ScoreSignals::interaction`]. Default `3.0` — the
    /// strongest signal, reflecting direct user engagement.
    pub interaction: f32,
    /// Multiplier for [`ScoreSignals::entity_density`]. Default `1.0`.
    pub entity_density: f32,
    /// Multiplier for [`ScoreSignals::llm_importance`]. Default `0.0`
    /// (disabled); see [`SignalWeights::with_llm_enabled`].
    pub llm_importance: f32,
}

impl Default for SignalWeights {
    fn default() -> Self {
        Self {
            token_count: 1.0,
            unique_words: 1.0,
            metadata_weight: 1.5,
            source_weight: 1.5,
            interaction: 3.0, // strongest signal — direct user engagement
            entity_density: 1.0,
            llm_importance: 0.0, // disabled until LLM extractor is configured
        }
    }
}

impl SignalWeights {
    /// Same as [`Default::default`] but with a non-zero `llm_importance` weight.
    /// Use when an LLM extractor is wired in and you want its importance
    /// signal to influence the admission decision.
    pub fn with_llm_enabled() -> Self {
        Self {
            llm_importance: 2.0,
            ..Self::default()
        }
    }
}

#[cfg(test)]
#[path = "types_tests.rs"]
mod tests;
