//! Header / metadata helpers: extract people from email-header values, detect a
//! document's primary subject, and merge parsed metadata back onto the input.

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};

use super::regex::{named_email_regex, sanitize_entity_name, sanitize_fact_text};
use super::types::{ExtractionAccumulator, MemoryIngestionConfig, ParsedIngestion};
use crate::memory::types::NamespaceDocumentInput;

/// Extract `Name <email>` people from a header value, recording them as PERSON
/// entities and returning their canonical names in order.
pub(super) fn extract_people_from_header(
    value: &str,
    accumulator: &mut ExtractionAccumulator,
) -> Vec<String> {
    let mut people = Vec::new();
    for captures in named_email_regex().captures_iter(value) {
        let name = sanitize_fact_text(
            captures
                .name("name")
                .map(|value| value.as_str())
                .unwrap_or(""),
        );
        if name.is_empty() {
            continue;
        }
        let canonical = sanitize_entity_name(&name);
        let _ = accumulator.add_entity(&canonical, "PERSON", 0.95);
        accumulator.remember_person_aliases(&canonical);
        people.push(canonical);
    }
    people
}

/// Detect a hard-coded primary subject. Currently only recognises OpenHuman.
pub(super) fn detect_primary_subject(text: &str) -> Option<String> {
    if text.contains("OpenHuman") {
        return Some("OPENHUMAN".to_string());
    }
    None
}

/// Merge parsed metadata + tags back onto the document input. Returns the
/// enriched input and the merged tag list.
///
/// Retained from the OpenHuman port as part of the deterministic enrichment
/// surface a host can use when persisting documents. TinyCortex's
/// [`extract_document`](super::extract_document) does extraction only and does
/// not persist, so this is currently exercised by tests rather than the hot
/// path.
#[allow(dead_code)]
pub(super) fn enrich_document_metadata(
    input: &NamespaceDocumentInput,
    parsed: &ParsedIngestion,
    config: &MemoryIngestionConfig,
) -> (NamespaceDocumentInput, Vec<String>) {
    let mut metadata = match input.metadata.clone() {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    for (key, value) in parsed.metadata.as_object().cloned().unwrap_or_default() {
        metadata.insert(key, value);
    }
    metadata.insert(
        "ingestion".to_string(),
        json!({
            "backend": "tinycortex_rust_heuristic",
            "model_name": config.model_name,
            "extraction_mode": config.extraction_mode.as_str(),
            "entity_count": parsed.entities.len(),
            "relation_count": parsed.relations.len(),
            "preference_count": parsed.preference_count,
            "decision_count": parsed.decision_count,
            "chunk_count": parsed.chunk_count,
        }),
    );
    if parsed.preference_count > 0 || parsed.decision_count > 0 {
        metadata.insert("kind".to_string(), json!("profile"));
    }

    let mut tags = input.tags.iter().cloned().collect::<BTreeSet<_>>();
    tags.extend(parsed.tags.iter().cloned());
    let tags = tags.into_iter().collect::<Vec<_>>();

    (
        NamespaceDocumentInput {
            namespace: input.namespace.clone(),
            key: input.key.clone(),
            title: input.title.clone(),
            content: input.content.clone(),
            source_type: input.source_type.clone(),
            priority: input.priority.clone(),
            tags: tags.clone(),
            metadata: Value::Object(metadata),
            category: input.category.clone(),
            session_id: input.session_id.clone(),
            document_id: input.document_id.clone(),
            // Carry the caller's provenance forward — parsing is a pure
            // metadata-enrichment step, so ingest taint must survive it.
            taint: input.taint,
        },
        tags,
    )
}
