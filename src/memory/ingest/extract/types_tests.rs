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

/// Pins the `MemoryIngestionResult` JSON literal-for-literal so the wire
/// shape (camelCase keys, field set) can never drift.
#[test]
fn ingestion_result_serialises_the_engine_shape() {
    let result = MemoryIngestionResult {
        document_id: "doc-1".into(),
        namespace: "global".into(),
        model_name: "driver-managed".into(),
        extraction_mode: "driver-managed".into(),
        chunk_count: 0,
        entity_count: 0,
        relation_count: 0,
        preference_count: 0,
        decision_count: 0,
        tags: vec!["t".into()],
        entities: Vec::new(),
        relations: Vec::new(),
    };
    let value = serde_json::to_value(&result).expect("serialise");
    assert_eq!(
        value,
        json!({
            "documentId": "doc-1",
            "namespace": "global",
            "modelName": "driver-managed",
            "extractionMode": "driver-managed",
            "chunkCount": 0,
            "entityCount": 0,
            "relationCount": 0,
            "preferenceCount": 0,
            "decisionCount": 0,
            "tags": ["t"],
            "entities": [],
            "relations": []
        })
    );
}

/// A config a prior engine accepted must still deserialize: `modelName`
/// required, everything else defaulted, including the mode's `snake_case`
/// spelling.
#[test]
fn ingestion_config_minimal_json_defaults_and_requires_model_name() {
    let config: MemoryIngestionConfig =
        serde_json::from_value(json!({ "modelName": "heuristic-only" }))
            .expect("minimal config deserialises");
    assert_eq!(config.extraction_mode, ExtractionMode::Sentence);
    assert!((config.entity_threshold - 0.45).abs() < f32::EPSILON);
    assert!((config.relation_threshold - 0.30).abs() < f32::EPSILON);
    assert!((config.adjacency_threshold - 0.50).abs() < f32::EPSILON);
    assert_eq!(config.batch_size, 16);
    let chunk: ExtractionMode = serde_json::from_value(json!("chunk")).expect("snake_case mode");
    assert_eq!(chunk, ExtractionMode::Chunk);
    assert!(serde_json::from_value::<MemoryIngestionConfig>(json!({})).is_err());
}
