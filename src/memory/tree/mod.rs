//! Summary-tree mechanics — ported from OpenHuman's `memory_tree` + the tree
//! row storage from `memory_store/trees`.
//!
//! Two engines live here:
//!
//! - **Bucket-seal SQLite trees** ([`store`], [`bucket_seal`], [`flush`],
//!   [`registry`], [`factory`], [`io`], [`read`], [`summarise`], [`hydrate`]).
//!   Source/topic/global trees keyed by `(kind, scope)` in `mem_tree_trees`.
//!   Leaves accumulate in per-`(tree, level)` buffers; when a buffer crosses its
//!   token (L0) or fan-in (L≥1) gate it seals into an immutable
//!   [`store::SummaryNode`], cascading upward. The LLM is abstracted behind
//!   [`summarise::Summariser`] (default deterministic [`summarise::ConcatSummariser`]);
//!   embedding is abstracted via the already-ported `score::embed` seam.
//!
//! - **Markdown time-tree** ([`runtime`]). A root → year → month → day → hour
//!   hierarchy persisted as markdown files, with its own [`runtime::Summariser`].
//!
//! ## Deferred (not ported here)
//!
//! Hybrid retrieval (separate `retrieval` module), tree health/doctor, NLP, the
//! co-occurrence graph, RPC/CLI/bus/schema surfaces, on-disk content staging +
//! git mirroring, the async-queue seal follow-ups, seal-time embedding, and the
//! per-document subtree (`seal_document_subtree`).

pub mod store;

pub mod bucket_seal;
mod direct_ingest;
pub mod factory;
pub mod flush;
mod hydrate;
pub mod io;
pub mod read;
pub mod registry;
pub mod runtime;
pub mod summarise;

// ── Public API surface ──────────────────────────────────────────────────────

pub use bucket_seal::{
    append_leaf, append_leaf_deferred, append_to_buffer, cascade_all_from,
    cascade_all_from_with_services, seal_document_subtree_with_services,
    seal_one_level_with_services, should_seal, LabelStrategy, LeafRef, SealObserver, SealServices,
    MERGE_LEVEL_BASE,
};
pub use direct_ingest::{ingest_summary, SummaryIngestInput, SummaryIngestOutcome};
pub use factory::{TreeFactory, TreeProfile, GLOBAL_SCOPE};
pub use flush::{
    flush_stale_buffers, flush_stale_buffers_default, flush_stale_buffers_with_services,
    force_flush_tree,
};
pub use hydrate::fetch_leaves;
pub use io::{
    TreeLabelStrategy, TreeLeafPayload, TreeReadHit, TreeReadRequest, TreeReadResult,
    TreeWriteOutcome, TreeWriteRequest,
};
pub use read::read_tree;
pub use registry::{get_or_create_tree, is_unique_violation, new_summary_id, new_tree_id};
pub use store::{
    Buffer, SummaryNode, Tree, TreeKind, TreeStatus, INPUT_TOKEN_BUDGET, OUTPUT_TOKEN_BUDGET,
    SUMMARY_FANOUT,
};
pub use summarise::{
    fallback_summary, finish_provider_summary, prepare_summary_prompt, ConcatSummariser,
    PreparedSummaryPrompt, Summariser, SummaryCall, SummaryContext, SummaryInput, SummaryOutput,
};
