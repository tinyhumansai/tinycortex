//! The TinyCortex memory engine.
//!
//! A layered, config-driven memory system ported from OpenHuman. The layering
//! rule from the spec holds: orchestration and ingestion depend on storage;
//! storage never depends upward on orchestration, tools, or agents.
//!
//! ## Layers
//!
//! - [`types`] / [`traits`] / [`config`] / [`error`]: stable shared contracts.
//! - [`store`]: storage primitives (content, chunks, trees, vectors, KV, …).
//! - [`chunks`]: canonical chunk model and deterministic ids.
//! - [`sources`]: source registry contracts and validation.
//! - [`score`]: scoring, entity extraction, and embedding signals.
//! - [`tree`]: summary-tree mechanics (append, seal, summarise, retrieve).
//! - [`queue`]: async job model (extract, append, seal, flush, backfill).
//! - [`retrieval`]: vector / keyword / graph / tree / hybrid search.
//! - `diff`: git-backed source snapshots, diffs, checkpoints, read markers
//!   (feature `git-diff`; gates the heavy native `git2`/libgit2 dependency).
//! - [`entities`] / [`graph`]: entity files and derived co-occurrence graph.
//! - [`goals`] / [`tool_memory`]: specialized long-term memory surfaces.
//! - [`conversations`] / [`archivist`]: transcript storage and tree archival.
//! - [`ingest`]: the canonicalize → chunk → score → tree ingest pipeline.
//!
//! ## Invariants
//!
//! - **Local-first workspace.** [`MemoryConfig::workspace`] is the single
//!   authoritative root: markdown content, SQLite indexes, and ledgers all live
//!   under it. Nothing in this crate treats a remote service as the source of
//!   truth; remote sync (Gmail, Slack, Notion, …) only ever writes through the
//!   same local pipeline as any other content.
//! - **Storage never depends upward.** `store`, `chunks`, and `sources` may be
//!   used by `tree`, `queue`, `retrieval`, `ingest`, and the higher-level
//!   surfaces, never the other way around. This keeps the storage layer testable
//!   and reusable without pulling in orchestration.
//! - **Crash-safe writes.** Anything that mutates on-disk state (trees, KV,
//!   markdown documents, ledgers) goes through [`fsutil`]'s atomic
//!   write-then-rename helpers so a crash mid-write never leaves a torn file
//!   readers can observe.
//! - **Provenance taint fails closed.** [`types::MemoryTaint`] defaults to
//!   [`types::MemoryTaint::Internal`] for first-class in-process content, but
//!   any unrecognised persisted value decodes as
//!   [`types::MemoryTaint::ExternalSync`] — the more restrictive setting — so
//!   policy gates never under-trust content of unknown provenance.
//! - **Feature-gated modules add no default-build cost.** `diff`, `providers`,
//!   and `rpc` are compiled out entirely unless their feature is enabled (see
//!   the crate-level feature-flag docs in `lib.rs`); code in this module must
//!   not assume they are present.

// ── Shared contracts ────────────────────────────────────────────────────────
pub mod config;
pub mod error;
pub mod traits;
pub mod types;

// ── Storage primitives ──────────────────────────────────────────────────────
pub mod store;

// ── Layered modules (ported incrementally from OpenHuman) ───────────────────
pub mod archivist;
pub mod chunks;
pub mod conversations;
/// Git-backed source snapshots, diffs, checkpoints, and read markers.
///
/// Gated behind the `git-diff` feature: the entire module (and the heavy native
/// `git2`/libgit2 dependency it needs) compiles out when the feature is off.
#[cfg(feature = "git-diff")]
pub mod diff;
pub mod entities;
/// Shared filesystem primitives (crash-safe atomic writes).
pub mod fsutil;
pub mod goals;
pub mod graph;
pub mod ingest;
pub mod queue;
pub mod retrieval;
pub mod score;
pub mod sources;
pub mod tool_memory;
pub mod tree;

// ── Feature-gated boundary surfaces ─────────────────────────────────────────
/// reqwest-based embedding / LLM HTTP providers.
///
/// Gated behind the `providers-http` feature (implies `tokio`). Reserves the
/// HTTP provider seam and gates the reqwest dependency; the concrete providers
/// land with goals C3/M3.
#[cfg(feature = "providers-http")]
pub mod providers;

/// serde schema / envelope surface for the RPC boundary.
///
/// Gated behind the `rpc` feature. Reserves the wire-facing surface for goal
/// C5 without adding heavy dependencies.
#[cfg(feature = "rpc")]
pub mod rpc;

// ── Re-exports ──────────────────────────────────────────────────────────────
pub use config::{MemoryConfig, WeightProfile};
pub use error::{MemoryEngineResult, MemoryError as MemoryEngineError};
pub use traits::Memory;
pub use types::{
    GraphRelationRecord, MemoryCategory, MemoryEntry, MemoryItemKind, MemoryKvRecord, MemoryTaint,
    NamespaceDocumentInput, NamespaceMemoryHit, NamespaceQueryResult, NamespaceRetrievalContext,
    NamespaceSummary, RecallOpts, RetrievalScoreBreakdown, StoredMemoryDocument, GLOBAL_NAMESPACE,
};

// Starter in-memory store API (kept stable for the smoke test and as a simple
// reference backend while richer backends are ported under `store`).
pub use store::types::{
    MemoryError, MemoryId, MemoryInput, MemoryQuery, MemoryRecord, MemoryResult, SearchHit,
};
pub use store::{InMemoryMemoryStore, MemoryStore};
