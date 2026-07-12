//! Hybrid retrieval primitives, ported from OpenHuman's `memory_search` and
//! `memory_tree::retrieval`.
//!
//! Each primitive is **deterministic and scope-specific**; orchestration (which
//! primitive to call, how to combine results) is left to the caller — there is
//! no classifier, gate, or composer here. Every primitive emits the same
//! unified [`RetrievalHit`] / [`QueryResponse`] shape so a caller sees one
//! schema regardless of which ran.
//!
//! ## Primitives
//!
//! - [`query_source`] — per-source-tree summary retrieval (optional semantic
//!   rerank).
//! - [`query_global`] — cross-source digest over an explicit time window,
//!   reconstructed from source-tree summaries.
//! - [`query_topic`] — entity/topic-scoped retrieval reconstructed from the
//!   entity index plus hydrated source-tree nodes.
//! - [`search_entities`] — fuzzy `LIKE` lookup over the entity index.
//! - [`drill_down`] — descend a summary's `child_ids` (BFS, optional rerank).
//! - [`cover_window`] — minimum-node cover of a `[since, until]` window.
//! - [`fetch_leaves`] — batch-hydrate raw chunk leaves by id, capped.
//!
//! ## Hybrid scoring (defined, not yet wired)
//!
//! [`scoring`] supplies the deterministic signal functions (keyword overlap,
//! freshness decay) and a composer ([`scoring::hybrid_score`]) that folds
//! graph / vector / keyword / freshness into a
//! [`crate::memory::types::RetrievalScoreBreakdown`] under the active
//! [`crate::memory::config::WeightProfile`] (read from config — never
//! hardcoded). [`mmr`] provides Maximal Marginal Relevance diversification.
//!
//! NOTE: as of this writing none of the primitives above call
//! `hybrid_score`, `keyword_relevance`, `freshness`, `mmr_select`, or the
//! [`graph_adapter`] path — every query above ranks purely by the stored
//! admission score, the internal semantic cosine rerank helper, or recency.
//! These functions are correct and unit-tested in isolation but are
//! dead code from the primitives' point of view until a caller composes
//! them. Treat this section as documenting *available building blocks*, not
//! *active behavior* — do not assume a query result reflects graph or
//! keyword relevance.
//!
//! ## Reuse
//!
//! These primitives delegate storage to the already-ported modules:
//! - [`crate::memory::tree`] for tree/summary reads,
//! - [`crate::memory::chunks`] for leaf hydration + the shared SQLite handle,
//! - [`crate::memory::score`] for embeddings ([`Embedder`]) and the entity
//!   index,
//! - [`crate::memory::graph`] for co-occurrence relevance (via
//!   [`graph_adapter::ConfigEntityIndex`], itself currently unused by any
//!   primitive here — see the note above).
//!
//! [`Embedder`]: crate::memory::score::embed::Embedder

pub mod cover;
pub mod drill_down;
pub mod fast;
pub mod fetch;
pub mod global;
pub mod graph_adapter;
pub mod mmr;
mod rerank;
pub mod scoring;
pub mod search;
pub mod source;
pub mod types;

#[cfg(test)]
#[path = "graph_adapter_tests.rs"]
mod graph_adapter_tests;
#[cfg(test)]
pub(crate) mod test_support;

// ── Public re-exports ───────────────────────────────────────────────────────

pub use cover::{cover_window, cover_window_scoped};
pub use drill_down::drill_down;
pub use fast::{fast_retrieve, FastRetrieveOptions};
pub use fetch::{fetch_leaves, MAX_BATCH};
pub use global::{query_global, query_topic};
pub use graph_adapter::ConfigEntityIndex;
pub use mmr::{mmr_select, MmrCandidate, MmrResult};
pub use scoring::{freshness, hybrid_score, keyword_relevance};
pub use search::search_entities;
pub use source::query_source;
pub use types::{
    hit_from_chunk, hit_from_summary, hit_from_summary_with_tree, leaf_tree_placeholder,
    EntityMatch, NodeKind, QueryResponse, RetrievalHit,
};
