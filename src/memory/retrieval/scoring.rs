//! Hybrid scoring: combine graph / vector / keyword / freshness signals into a
//! single ranking score under the active [`WeightProfile`].
//!
//! The weight profiles themselves (`balanced`, `semantic`, `lexical`,
//! `graph_first`) live in [`crate::memory::config`] and are read from config —
//! never hardcoded here. This module supplies the deterministic *signal*
//! functions (keyword overlap, freshness decay) and the composition that folds
//! them into a [`RetrievalScoreBreakdown`] so a caller can both rank and
//! explain why a hit ranked where it did.

use crate::memory::config::WeightProfile;
use crate::memory::types::RetrievalScoreBreakdown;

/// Default freshness half-life (days). A hit updated this many days ago scores
/// `0.5` on the freshness axis; one updated now scores `1.0`.
pub const DEFAULT_FRESHNESS_HALF_LIFE_DAYS: f64 = 7.0;

/// Milliseconds in a day.
const MS_PER_DAY: f64 = 86_400_000.0;

/// Lexical relevance of `content` to `query` in `[0.0, 1.0]`.
///
/// Computed as the fraction of distinct lowercased query tokens that appear as
/// distinct tokens in the content. An empty query (or content) scores `0.0`.
/// This is deliberately simple and dependency-free — it is a keyword signal,
/// not a ranking function on its own.
pub fn keyword_relevance(query: &str, content: &str) -> f64 {
    let q_tokens: std::collections::HashSet<String> = tokenize(query).collect();
    if q_tokens.is_empty() {
        return 0.0;
    }
    let c_tokens: std::collections::HashSet<String> = tokenize(content).collect();
    if c_tokens.is_empty() {
        return 0.0;
    }
    let matched = q_tokens.iter().filter(|t| c_tokens.contains(*t)).count();
    matched as f64 / q_tokens.len() as f64
}

/// Lowercase, alphanumeric-token splitter shared by the keyword signal.
fn tokenize(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_lowercase())
}

/// Freshness of a hit in `[0.0, 1.0]` via exponential half-life decay.
///
/// `now_ms` and `updated_at_ms` are epoch milliseconds. A hit at `now`
/// scores `1.0`; one `half_life_days` old scores `0.5`. Future timestamps
/// (clock skew) clamp to `1.0`; a non-positive half-life degrades to a hard
/// `1.0` (no decay).
pub fn freshness(updated_at_ms: i64, now_ms: i64, half_life_days: f64) -> f64 {
    if half_life_days <= 0.0 {
        return 1.0;
    }
    let age_days = (now_ms - updated_at_ms) as f64 / MS_PER_DAY;
    if age_days <= 0.0 {
        return 1.0;
    }
    0.5_f64.powf(age_days / half_life_days)
}

/// Compose a [`RetrievalScoreBreakdown`] from the four raw signals under
/// `profile`. Each signal is expected in `[0.0, 1.0]`; the final score is the
/// weighted sum `graph·g + vector·v + keyword·k + freshness·f`.
///
/// `episodic_relevance` is left at `0.0` — episodic memory is not a tree-
/// retrieval signal — but is carried in the breakdown for wire compatibility
/// with [`RetrievalScoreBreakdown`].
pub fn hybrid_score(
    profile: &WeightProfile,
    graph_relevance: f64,
    vector_similarity: f64,
    keyword_relevance: f64,
    freshness: f64,
) -> RetrievalScoreBreakdown {
    let final_score = profile.graph * graph_relevance
        + profile.vector * vector_similarity
        + profile.keyword * keyword_relevance
        + profile.freshness * freshness;
    RetrievalScoreBreakdown {
        keyword_relevance,
        vector_similarity,
        graph_relevance,
        episodic_relevance: 0.0,
        freshness,
        final_score,
    }
}

#[cfg(test)]
#[path = "scoring_tests.rs"]
mod tests;
