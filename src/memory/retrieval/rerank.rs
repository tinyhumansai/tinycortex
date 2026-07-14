//! Shared semantic-rerank helper used by `query_source`, `query_global`,
//! `query_topic`, and `drill_down`.
//!
//! Each hit is decorated with the cosine similarity between the query embedding
//! and the hit's stored embedding. Hits with no embedding (legacy rows, or
//! leaves whose chunk was never embedded) sort to the bottom.
//!
//! Un-embedded hits sort after embedded hits while preserving their incoming
//! order. Equal-similarity embedded hits use recency as a deterministic tie
//! break.
//!
//! Embedding failures (e.g. a local model being unavailable) never surface as
//! an error to the caller: the helper logs nothing (per repo rules) and falls
//! back to returning `hits` completely unsorted (the original incoming order),
//! since the failure is detected before any decoration or sort happens.

use crate::memory::score::embed::{cosine_similarity, Embedder};

use super::types::RetrievalHit;

/// Rerank `hits` by cosine similarity to `query`'s embedding.
///
/// `embeddings[i]` is the stored vector for `hits[i]` (or `None` when the hit
/// has no embedding). The two slices MUST be the same length (checked by a
/// `debug_assert_eq!` — a release-mode mismatch degrades to zipping the
/// shorter of the two rather than panicking, since [`Iterator::zip`] stops at
/// the shorter side).
///
/// Ordering: embedded hits sort before un-embedded hits; similarity ties among
/// embedded hits break on `time_range_end` DESC, while un-embedded hits retain
/// input order. On any embed failure (e.g. `embedder.embed` erroring) `hits` is
/// returned as-is, in its original incoming order, with no decoration or
/// sorting attempted.
pub(crate) async fn rerank_by_semantic_similarity(
    embedder: &dyn Embedder,
    query: &str,
    hits: Vec<RetrievalHit>,
    embeddings: Vec<Option<Vec<f32>>>,
) -> Vec<RetrievalHit> {
    debug_assert_eq!(hits.len(), embeddings.len());
    let query_vec = match embedder.embed(query).await {
        Ok(v) => v,
        Err(_) => return hits,
    };

    // Decorate each hit with (similarity, has_embedding). Un-embedded rows get
    // `NEG_INFINITY` so they sort last but keep their incoming relative order.
    let mut decorated: Vec<(f32, bool, usize, RetrievalHit)> = hits
        .into_iter()
        .zip(embeddings)
        .enumerate()
        .map(|(index, (h, emb))| match emb {
            Some(v) if v.len() == query_vec.len() => {
                let sim = cosine_similarity(&query_vec, &v);
                (sim, true, index, h)
            }
            _ => (f32::NEG_INFINITY, false, index, h),
        })
        .collect();

    decorated.sort_by(|a, b| match (a.1, b.1) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        (false, false) => a.2.cmp(&b.2),
        (true, true) => {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.3.time_range_end.cmp(&a.3.time_range_end))
        }
    });

    decorated.into_iter().map(|(_, _, _, h)| h).collect()
}

#[cfg(test)]
#[path = "rerank_tests.rs"]
mod tests;
