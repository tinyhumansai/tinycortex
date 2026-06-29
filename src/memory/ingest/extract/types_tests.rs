use super::*;
use serde_json::json;

#[test]
fn extraction_mode_default_is_sentence() {
    assert_eq!(ExtractionMode::default(), ExtractionMode::Sentence);
    assert_eq!(ExtractionMode::Sentence.as_str(), "sentence");
    assert_eq!(ExtractionMode::Chunk.as_str(), "chunk");
}

#[test]
fn memory_ingestion_config_default_matches_expected_thresholds() {
    let cfg = MemoryIngestionConfig::default();
    assert_eq!(cfg.model_name, DEFAULT_MEMORY_EXTRACTION_MODEL);
    assert_eq!(cfg.extraction_mode, ExtractionMode::Sentence);
    assert_eq!(cfg.entity_threshold, 0.45);
    assert_eq!(cfg.relation_threshold, 0.30);
    assert_eq!(cfg.adjacency_threshold, 0.50);
    assert_eq!(cfg.batch_size, 16);
}

#[test]
fn memory_ingestion_request_defaults_config_when_absent() {
    let request: MemoryIngestionRequest = serde_json::from_value(json!({
        "document": {
            "namespace": "global",
            "key": "doc-1",
            "title": "Doc",
            "content": "Body",
            "source_type": "manual",
            "priority": "normal",
            "category": "core"
        }
    }))
    .unwrap();
    assert_eq!(request.config.model_name, DEFAULT_MEMORY_EXTRACTION_MODEL);
    assert_eq!(request.config.extraction_mode, ExtractionMode::Sentence);
}
