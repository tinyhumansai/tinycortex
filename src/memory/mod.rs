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
//! - `diff`: git-backed source snapshots, diffs, checkpoints, read markers.
//!   Its inert `types`/`source` submodules are always compiled; computing a
//!   diff needs feature `git-diff`, which gates the heavy native
//!   `git2`/libgit2 dependency.
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
//! - **Feature-gated modules add no default-build cost.** `providers` and
//!   `persona` are compiled out entirely unless their feature is enabled, and
//!   `diff` keeps only its inert `types`/`source` submodules without `git-diff`
//!   (see the crate-level feature-flag docs in `lib.rs`). Code in this module
//!   must not assume any of them is present.

// ── Shared contracts ────────────────────────────────────────────────────────
pub mod config;
/// The stable value types, error enum, and storage trait now live in the
/// dependency-light `tinycortex-api` crate. Re-exporting the modules (rather
/// than the individual items) keeps every `memory::types::…` /
/// `super::types::…` path inside this crate — and in embedding hosts —
/// resolving exactly as before.
pub use tinycortex_api::{error, traits, types};

// ── Storage primitives ──────────────────────────────────────────────────────
pub mod store;

// ── Layered modules (ported incrementally from OpenHuman) ───────────────────
pub mod archivist;
pub mod chunks;
pub mod conversations;
/// Git-backed source snapshots, diffs, checkpoints, and read markers.
///
/// Always compiled, but mostly hollow without the `git-diff` feature: the inert
/// `types` and `source` submodules are `serde`/`std`-only and stay available so
/// a host can still *describe* a diff, while everything that computes one —
/// `Ledger`, `DiffEngine`, and the checkpoint/snapshot/diff operations — is
/// gated along with the heavy native `git2`/libgit2 dependency it needs. See
/// the module's own docs for why the split falls where it does.
pub mod diff;
pub mod entities;
/// Shared filesystem primitives (crash-safe atomic writes).
pub mod fsutil;
pub mod goals;
pub mod graph;
pub mod health;
pub mod ingest;
pub mod queue;
pub mod retrieval;
pub mod score;
pub mod sources;
/// Live synchronization engine. Network code is opt-in through `sync`.
#[cfg(feature = "sync")]
pub mod sync;
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

/// Persona distillation: turns local coding-agent history, instruction files,
/// and git commits into a durable persona memory layer (doc 06).
///
/// Gated behind the default-off `persona` feature. Adds no new dependencies;
/// the git-history reader additionally requires `git-diff`.
#[cfg(feature = "persona")]
pub mod persona;
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
    MemoryId, MemoryInput, MemoryQuery, MemoryRecord, MemoryResult, SearchHit, StoreError,
};
pub use store::{InMemoryMemoryStore, MemoryStore};
