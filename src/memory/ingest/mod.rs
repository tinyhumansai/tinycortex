//! The on-demand ingest pipeline.
//!
//! Ported from OpenHuman's `memory_sync/canonicalize`, `memory/ingest_pipeline`,
//! and the deterministic core of `memory/ingestion`. TinyCortex does **not**
//! own live memory sync; this module assumes the host supplies a source-scoped
//! payload and runs the path:
//!
//! ```text
//! canonicalize -> write raw markdown -> chunk -> score/extract
//!   -> persist chunk metadata -> enqueue tree jobs (-> append/seal in worker)
//! ```
//!
//! ## Layout
//!
//! - [`canonicalize`] — chat / email / document → [`CanonicalisedSource`].
//! - [`pipeline`]     — the orchestration ([`pipeline::ingest_canonical`] plus
//!   per-kind convenience wrappers) that chunks, scores, persists, and enqueues.
//! - [`extract`]      — the deterministic heuristic extractor (parse / regex /
//!   rules) recovering entities & relations from document text.
//! - [`types`]        — the [`TreeJobSink`] seam, [`IngestOptions`], and
//!   [`IngestSummary`].
//!
//! ## Ownership boundary
//!
//! The async job queue is ported separately, so the orchestrator injects the
//! tree-job enqueue behind [`TreeJobSink`] rather than hard-depending on
//! `crate::memory::queue`. Buffer append and summary seal run in the async
//! extract worker (driven off the sink), not on this hot path. The live sync
//! runner/scheduler and the namespace document/graph store are out of scope and
//! intentionally not ported.

pub mod canonicalize;
pub mod extract;
pub mod pipeline;
pub mod types;

pub use canonicalize::chat::{ChatBatch, ChatMessage};
pub use canonicalize::document::DocumentInput;
pub use canonicalize::email::{EmailMessage, EmailThread};
pub use canonicalize::{CanonicaliseRequest, CanonicalisedSource};

pub use extract::{
    extract_document, ExtractedEntity, ExtractedRelation, ExtractionMode, MemoryIngestionConfig,
    MemoryIngestionRequest, MemoryIngestionResult, DEFAULT_MEMORY_EXTRACTION_MODEL,
};

pub use pipeline::{
    ingest_canonical, ingest_chat, ingest_document, ingest_document_versioned,
    ingest_document_with_scope, ingest_email, ingest_email_with_raw_refs,
};
pub use types::{IngestOptions, IngestSummary, NullJobSink, TreeJobSink};
