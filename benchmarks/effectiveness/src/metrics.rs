//! Ranking-quality metrics for retrieval evaluation.
//!
//! All metrics operate on a single query's **ranked** predictions — a list of
//! document ids ordered most-relevant-first as returned by a backend — scored
//! against the query's **relevant** set (binary relevance / ground truth). They
//! are pure functions with no I/O so they are cheap to unit-test and reuse.
//!
//! Conventions:
//! - `k` is a 1-based cutoff ("top-k"); `k == 0` yields `0.0`.
//! - Ids beyond position `k` are ignored.
//! - An empty relevant set yields `0.0` for recall-style metrics (there is
//!   nothing to find), matching how most IR toolkits skip such queries.

use std::collections::HashSet;

/// Count how many of the top-`k` predictions are in the `relevant` set.
fn hits_in_top_k(ranked: &[String], relevant: &HashSet<String>, k: usize) -> usize {
    ranked
        .iter()
        .take(k)
        .filter(|id| relevant.contains(*id))
        .count()
}

/// Recall@k — fraction of all relevant docs retrieved within the top-`k`.
///
/// `|relevant ∩ top_k| / |relevant|`. Returns `0.0` when `relevant` is empty.
pub fn recall_at_k(ranked: &[String], relevant: &HashSet<String>, k: usize) -> f64 {
    if relevant.is_empty() {
        return 0.0;
    }
    hits_in_top_k(ranked, relevant, k) as f64 / relevant.len() as f64
}

/// Precision@k — fraction of the top-`k` predictions that are relevant.
///
/// `|relevant ∩ top_k| / k`. Returns `0.0` when `k == 0`.
pub fn precision_at_k(ranked: &[String], relevant: &HashSet<String>, k: usize) -> f64 {
    if k == 0 {
        return 0.0;
    }
    hits_in_top_k(ranked, relevant, k) as f64 / k as f64
}

/// Hit@k — `1.0` if any relevant doc appears in the top-`k`, else `0.0`.
pub fn hit_at_k(ranked: &[String], relevant: &HashSet<String>, k: usize) -> f64 {
    if hits_in_top_k(ranked, relevant, k) > 0 {
        1.0
    } else {
        0.0
    }
}

/// Reciprocal rank — `1 / rank` of the first relevant hit (rank is 1-based),
/// or `0.0` if none of `ranked` is relevant. Averaged across queries this is MRR.
pub fn reciprocal_rank(ranked: &[String], relevant: &HashSet<String>) -> f64 {
    for (index, id) in ranked.iter().enumerate() {
        if relevant.contains(id) {
            return 1.0 / (index as f64 + 1.0);
        }
    }
    0.0
}

/// nDCG@k with binary gains.
///
/// DCG uses the standard `1 / log2(rank + 1)` discount (rank 1-based); the ideal
/// DCG assumes every relevant doc sits at the top. Returns `0.0` when there is
/// no attainable gain (empty relevant set) so it composes cleanly in averages.
pub fn ndcg_at_k(ranked: &[String], relevant: &HashSet<String>, k: usize) -> f64 {
    let dcg: f64 = ranked
        .iter()
        .take(k)
        .enumerate()
        .map(|(index, id)| {
            if relevant.contains(id) {
                1.0 / (index as f64 + 2.0).log2()
            } else {
                0.0
            }
        })
        .sum();

    let ideal_hits = relevant.len().min(k);
    let idcg: f64 = (0..ideal_hits)
        .map(|index| 1.0 / (index as f64 + 2.0).log2())
        .sum();

    if idcg == 0.0 {
        0.0
    } else {
        dcg / idcg
    }
}

#[cfg(test)]
#[path = "metrics_tests.rs"]
mod tests;
