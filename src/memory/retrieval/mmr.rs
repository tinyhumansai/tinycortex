//! Maximal Marginal Relevance (MMR) selection.
//!
//! Given a set of candidate vectors and a query vector, select a diverse subset
//! that balances relevance to the query against redundancy within the selected
//! set. Ported from OpenHuman's `memory_search::vector::mmr`. Cosine similarity
//! is reused from [`crate::memory::store::vectors`].
//!
//! NOTE: this module is one of two divergent `cosine_similarity`
//! implementations in the crate. The one used here
//! ([`crate::memory::store::vectors::cosine_similarity`]) is `f64` and clamps
//! its result to `[0.0, 1.0]`; the one used by the semantic-rerank helper
//! (`crate::memory::score::embed::cosine_similarity`) is `f32` and can be
//! negative. MMR selection here therefore cannot distinguish an
//! anti-correlated candidate from an orthogonal one — both clamp to `0.0`
//! similarity to the already-selected set.

use crate::memory::store::vectors::cosine_similarity;

/// A candidate for MMR selection.
pub struct MmrCandidate<'a> {
    /// Caller-side index, echoed back on the result so the candidate can be
    /// resolved to its original record.
    pub index: usize,
    /// Candidate embedding; must share dimensionality with the query vector and
    /// every other candidate, since cosine similarity is computed pairwise.
    pub embedding: &'a [f32],
    /// Precomputed relevance of this candidate to the query (typically a cosine
    /// score). Higher is more relevant; weighted by `lambda` in the MMR formula.
    pub relevance: f64,
}

/// Result of MMR selection: the original index and its MMR score.
#[derive(Debug, Clone)]
pub struct MmrResult {
    /// Caller-side index echoed from the chosen [`MmrCandidate::index`], used to
    /// resolve the result back to its original record.
    pub index: usize,
    /// The MMR score at the step this item was selected:
    /// `lambda · relevance − (1 − lambda) · max_similarity(c, selected)`.
    /// Not comparable across runs with different `lambda`.
    pub score: f64,
}

/// Select up to `limit` items from `candidates` using MMR.
///
/// `lambda` controls the relevance-diversity tradeoff:
/// - `1.0` = pure relevance (no diversity)
/// - `0.0` = pure diversity (ignores relevance)
/// - `0.7` = recommended default
///
/// For each selection step:
/// `mmr(c) = lambda · relevance(c) − (1 − lambda) · max_similarity(c, selected)`.
///
/// `lambda` is clamped to `[0.0, 1.0]`; `limit` is clamped to
/// `candidates.len()`. Returns `Vec::new()` immediately if `candidates` is
/// empty or `limit == 0`.
///
/// # Gotcha: `query_vec` is currently unused
///
/// `query_vec` is accepted but not read — relevance-to-query is taken
/// entirely from each candidate's precomputed [`MmrCandidate::relevance`]
/// field, which the caller must have already derived from the query
/// (typically via the same cosine similarity used here). Passing a
/// mismatched or empty `query_vec` has no effect on the result; this
/// parameter exists for API-shape parity with callers that have the query
/// vector in hand and may be wired to something in a future revision.
pub fn mmr_select(
    query_vec: &[f32],
    candidates: &[MmrCandidate<'_>],
    limit: usize,
    lambda: f64,
) -> Vec<MmrResult> {
    if candidates.is_empty() || limit == 0 {
        return Vec::new();
    }

    let lambda = lambda.clamp(0.0, 1.0);
    let limit = limit.min(candidates.len());

    let mut selected_embeddings: Vec<&[f32]> = Vec::with_capacity(limit);
    let mut results: Vec<MmrResult> = Vec::with_capacity(limit);
    let mut available: Vec<bool> = vec![true; candidates.len()];

    for _ in 0..limit {
        let mut best_idx: Option<usize> = None;
        let mut best_mmr = f64::NEG_INFINITY;

        for (i, candidate) in candidates.iter().enumerate() {
            if !available[i] {
                continue;
            }
            let max_sim_to_selected = if selected_embeddings.is_empty() {
                0.0
            } else {
                selected_embeddings
                    .iter()
                    .map(|sel| cosine_similarity(candidate.embedding, sel))
                    .fold(0.0_f64, f64::max)
            };
            let mmr_score = lambda * candidate.relevance - (1.0 - lambda) * max_sim_to_selected;
            if mmr_score > best_mmr {
                best_mmr = mmr_score;
                best_idx = Some(i);
            }
        }

        let Some(idx) = best_idx else { break };
        available[idx] = false;
        selected_embeddings.push(candidates[idx].embedding);
        results.push(MmrResult {
            index: candidates[idx].index,
            score: best_mmr,
        });
    }

    let _ = query_vec;
    results
}

#[cfg(test)]
#[path = "mmr_tests.rs"]
mod tests;
