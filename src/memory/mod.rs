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
//! - [`diff`]: git-backed source snapshots, diffs, checkpoints, read markers.
//! - [`entities`] / [`graph`]: entity files and derived co-occurrence graph.
//! - [`goals`] / [`tool_memory`]: specialized long-term memory surfaces.
//! - [`conversations`] / [`archivist`]: transcript storage and tree archival.
//! - [`ingest`]: the canonicalize → chunk → score → tree ingest pipeline.

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
pub mod diff;
pub mod entities;
pub mod goals;
pub mod graph;
pub mod ingest;
pub mod queue;
pub mod retrieval;
pub mod score;
pub mod sources;
pub mod tool_memory;
pub mod tree;

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
