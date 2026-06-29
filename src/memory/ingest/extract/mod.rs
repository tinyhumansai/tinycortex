//! Deterministic document extraction.
//!
//! A faithful port of the **parse / regex / rules** core of OpenHuman's
//! `memory/ingestion` pipeline. It takes raw unstructured text and recovers
//! structured knowledge with no model calls:
//!
//! 1. **Chunking** — split the document into manageable pieces ([`chunking`],
//!    [`text`]).
//! 2. **Structured extraction** — regex rules for email headers, prefixed
//!    fields, and explicit graph facts ([`regex`], [`parse_lines`],
//!    [`parse_relations`]).
//! 3. **Heuristic extraction** — recipient / spatial relations over extraction
//!    units ([`parse_units`]).
//! 4. **Aggregation** — alias resolution, dedup, thresholding ([`alias`],
//!    [`aggregate`], [`rules`]).
//!
//! ## Ownership boundary
//!
//! TinyCortex does **not** own the namespace document/graph store, so the
//! `impl UnifiedMemory` glue (document upsert + graph relation writes) and the
//! live-ingestion singleton runner state are intentionally **not** ported. The
//! public [`extract_document`] entry point performs extraction only and returns
//! a fully populated [`MemoryIngestionResult`]; persistence is the host's job.

mod aggregate;
mod alias;
mod chunking;
mod header;
mod parse;
mod parse_lines;
mod parse_relations;
#[path = "parse_units.rs"]
mod parse_units;
mod regex;
mod rules;
mod text;
mod types;

pub use parse::extract_document;
pub use types::{
    ExtractedEntity, ExtractedRelation, ExtractionMode, MemoryIngestionConfig,
    MemoryIngestionRequest, MemoryIngestionResult, DEFAULT_MEMORY_EXTRACTION_MODEL,
};

#[cfg(test)]
#[path = "extract_tests.rs"]
mod extract_tests;
