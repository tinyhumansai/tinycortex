//! End-to-end deterministic-extraction tests over the gmail + notion fixtures.
//!
//! Adapted from OpenHuman's `memory/ingestion/tests.rs`. The original drove the
//! full `UnifiedMemory::ingest_document` path and asserted on namespace query /
//! recall / graph reads; those storage surfaces are out of TinyCortex's
//! ownership boundary. Here we assert directly on the deterministic extractor's
//! recovered entities, relations, preferences, and decisions.

use super::{extract_document, extract_enriched_document, ExtractionMode, MemoryIngestionConfig};
use crate::memory::types::{MemoryTaint, NamespaceDocumentInput};

fn fixture(path: &str) -> String {
    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(
        base.join("tests")
            .join("fixtures")
            .join("ingestion")
            .join(path),
    )
    .expect("fixture should load")
}

#[test]
fn gmail_fixture_extraction_recovers_required_signals() {
    let content = fixture("gmail_thread_example.txt");
    let result = extract_document(
        &content,
        "Memory integration plan for OpenHuman desktop",
        &MemoryIngestionConfig::default(),
    );

    assert!(result.entities.iter().any(|e| e.name == "SANIL JAIN"));
    assert!(result.entities.iter().any(|e| e.name == "RAVI KULKARNI"));
    assert!(result.entities.iter().any(|e| e.name == "ASHA MEHTA"));
    assert!(result.entities.iter().any(|e| e.name == "OPENHUMAN"));
    assert!(result.relations.iter().any(|r| r.subject == "OPENHUMAN"
        && r.predicate == "USES"
        && r.object.contains("JSON-RPC")));
    assert!(result
        .relations
        .iter()
        .any(|r| r.subject == "RAVI KULKARNI" && r.predicate == "OWNS"));
    assert!(result.preference_count >= 1);
    assert!(result.decision_count >= 1);
}

#[test]
fn notion_fixture_extraction_recovers_required_signals() {
    let content = fixture("notion_page_example.txt");
    let result = extract_document(
        &content,
        "OpenHuman Memory Layer Roadmap",
        &MemoryIngestionConfig::default(),
    );

    assert!(result.entities.iter().any(|e| e.name == "OPENHUMAN"));
    assert!(result.entities.iter().any(|e| e.name == "SANIL JAIN"));
    assert!(result.relations.iter().any(|r| r.subject == "OPENHUMAN"
        && r.predicate == "USES"
        && r.object.contains("JSON-RPC")));
    assert!(result
        .relations
        .iter()
        .any(|r| r.subject == "CORE CONTRACT LOCKED" && r.predicate == "HAS_DEADLINE"));
    assert!(result
        .relations
        .iter()
        .any(|r| r.subject == "SANIL JAIN" && r.predicate == "PREFERS"));
    assert!(result.preference_count >= 1);
    assert!(result.decision_count >= 1);
}

#[test]
fn chunk_mode_extraction_recovers_spatial_relations_and_recipient_units() {
    let config = MemoryIngestionConfig {
        extraction_mode: ExtractionMode::Chunk,
        relation_threshold: 0.1,
        ..MemoryIngestionConfig::default()
    };
    let result = extract_document(
        "Alice Smith sent the launch checklist to Bob Jones. \
         Kitchen is north of Garden. East Room is east of Office. \
         West Room is west of Kitchen. Garden is south of Kitchen.",
        "Office map handoff",
        &config,
    );

    assert!(result.entities.iter().any(|e| e.name == "ALICE SMITH"));
    assert!(result.entities.iter().any(|e| e.name == "BOB JONES"));
    assert!(result
        .relations
        .iter()
        .any(|r| { r.subject == "KITCHEN" && r.predicate == "NORTH_OF" && r.object == "GARDEN" }));
    assert!(result
        .relations
        .iter()
        .any(|r| { r.subject == "GARDEN" && r.predicate == "SOUTH_OF" && r.object == "KITCHEN" }));
}

#[test]
fn sentence_mode_extraction_recovers_east_and_west_spatial_relations() {
    let config = MemoryIngestionConfig {
        relation_threshold: 0.1,
        ..MemoryIngestionConfig::default()
    };
    let result = extract_document(
        "East Room is east of Office. West Room is west of Kitchen.",
        "Office directions",
        &config,
    );

    assert!(result
        .relations
        .iter()
        .any(|r| { r.subject == "EAST ROOM" && r.predicate == "EAST_OF" && r.object == "OFFICE" }));
    assert!(result
        .relations
        .iter()
        .any(|r| { r.subject == "OFFICE" && r.predicate == "WEST_OF" && r.object == "EAST ROOM" }));
    assert!(result.relations.iter().any(|r| {
        r.subject == "WEST ROOM" && r.predicate == "WEST_OF" && r.object == "KITCHEN"
    }));
    assert!(result.relations.iter().any(|r| {
        r.subject == "KITCHEN" && r.predicate == "EAST_OF" && r.object == "WEST ROOM"
    }));
}

#[test]
fn enriched_document_merges_header_metadata_and_reports_counts() {
    let input = NamespaceDocumentInput {
        namespace: "inbox".into(),
        key: "message-1".into(),
        title: "Project handoff".into(),
        content: "From: Alice Smith <alice@example.com>\nTags: launch, urgent\n\nAlice Smith owns Project Atlas.".into(),
        source_type: "email".into(),
        priority: "normal".into(),
        tags: vec!["existing".into()],
        metadata: serde_json::json!({}),
        category: "episodic".into(),
        session_id: None,
        document_id: None,
        taint: MemoryTaint::ExternalSync,
    };

    let (enriched, result) = extract_enriched_document(&input, &MemoryIngestionConfig::default());

    assert_eq!(enriched.title, input.title);
    assert!(enriched.tags.contains(&"existing".to_string()));
    assert_eq!(result.document_id, "");
    assert_eq!(result.namespace, "");
    assert!(result.entity_count > 0);
    assert_eq!(result.entity_count, result.entities.len());
    assert_eq!(result.relation_count, result.relations.len());
    assert_eq!(result.tags, enriched.tags);
}
