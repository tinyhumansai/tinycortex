//! Top-level deterministic document parser: chunk the content, drive the
//! per-line and per-unit extraction passes, then aggregate into a result.

use super::header::detect_primary_subject;
use super::parse_lines::process_content_lines;
use super::parse_units::process_units;
use super::regex::sanitize_entity_name;
use super::text::chunk_document_content;
use super::types::{
    ExtractionAccumulator, MemoryIngestionConfig, MemoryIngestionResult, ParsedIngestion,
    DEFAULT_CHUNK_TOKENS,
};

/// Run the heuristic extraction pipeline over `content` and return the parsed
/// entities, relations, tags, and metadata.
pub(super) fn parse_document(
    content: &str,
    title: &str,
    config: &MemoryIngestionConfig,
) -> ParsedIngestion {
    let chunks = chunk_document_content(content, DEFAULT_CHUNK_TOKENS);

    let mut accumulator = ExtractionAccumulator {
        document_title: Some(sanitize_entity_name(title)),
        primary_subject: detect_primary_subject(title),
        ..ExtractionAccumulator::default()
    };

    process_content_lines(content, &chunks, &mut accumulator, config);
    process_units(&chunks, &mut accumulator, config);

    super::aggregate::finalize(accumulator, config, chunks.len())
}

/// Run the deterministic extractor over a document and return a
/// [`MemoryIngestionResult`].
///
/// TinyCortex does not own the namespace document/graph store, so this entry
/// point performs **extraction only** — it does not persist documents or write
/// graph relations. `document_id` / `namespace` are left empty; the host is
/// responsible for persistence if desired. The recovered entities, relations,
/// preference/decision counts, and tags are fully populated.
pub fn extract_document(
    content: &str,
    title: &str,
    config: &MemoryIngestionConfig,
) -> MemoryIngestionResult {
    let parsed = parse_document(content, title, config);
    MemoryIngestionResult {
        document_id: String::new(),
        namespace: String::new(),
        model_name: config.model_name.clone(),
        extraction_mode: config.extraction_mode.as_str().to_string(),
        chunk_count: parsed.chunk_count,
        entity_count: parsed.entities.len(),
        relation_count: parsed.relations.len(),
        preference_count: parsed.preference_count,
        decision_count: parsed.decision_count,
        tags: parsed.tags,
        entities: parsed.entities,
        relations: parsed.relations,
    }
}

#[cfg(test)]
#[path = "parse_tests.rs"]
mod parse_tests;
