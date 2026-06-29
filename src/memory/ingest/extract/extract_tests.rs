//! End-to-end deterministic-extraction tests over the gmail + notion fixtures.
//!
//! Adapted from OpenHuman's `memory/ingestion/tests.rs`. The original drove the
//! full `UnifiedMemory::ingest_document` path and asserted on namespace query /
//! recall / graph reads; those storage surfaces are out of TinyCortex's
//! ownership boundary. Here we assert directly on the deterministic extractor's
//! recovered entities, relations, preferences, and decisions.

use super::{extract_document, MemoryIngestionConfig};

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
