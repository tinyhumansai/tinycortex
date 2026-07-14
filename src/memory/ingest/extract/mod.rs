//! Deterministic document extraction.
//!
//! A faithful port of the **parse / regex / rules** core of OpenHuman's
//! `memory/ingestion` pipeline. It takes raw unstructured text and recovers
//! structured knowledge with no model calls:
//!
//! 1. **Chunking** — split the document into manageable pieces (`chunking`,
//!    `text`).
//! 2. **Structured extraction** — regex rules for email headers, prefixed
//!    fields, and explicit graph facts (`regex`, `parse_lines`,
//!    `parse_relations`).
//! 3. **Heuristic extraction** — recipient / spatial relations over extraction
//!    units (`parse_units`).
//! 4. **Aggregation** — alias resolution, dedup, thresholding (`alias`,
//!    `aggregate`, `rules`).
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

use crate::memory::types::NamespaceDocumentInput;

/// Extract structured knowledge and merge deterministic ingestion metadata
/// into a document ready for a host-owned namespace store.
pub fn extract_enriched_document(
    input: &NamespaceDocumentInput,
    config: &MemoryIngestionConfig,
) -> (NamespaceDocumentInput, MemoryIngestionResult) {
    let parsed = parse::parse_document(&input.content, &input.title, config);
    let (enriched, tags) = header::enrich_document_metadata(input, &parsed, config);
    let result = MemoryIngestionResult {
        document_id: String::new(),
        namespace: String::new(),
        model_name: config.model_name.clone(),
        extraction_mode: config.extraction_mode.as_str().to_string(),
        chunk_count: parsed.chunk_count,
        entity_count: parsed.entities.len(),
        relation_count: parsed.relations.len(),
        preference_count: parsed.preference_count,
        decision_count: parsed.decision_count,
        tags,
        entities: parsed.entities,
        relations: parsed.relations,
    };
    (enriched, result)
}

#[cfg(test)]
#[path = "extract_tests.rs"]
mod extract_tests;
