//! Unit tests for the core memory data contracts in [`super`].

use super::*;
use serde_json::json;

#[test]
fn global_namespace_constant_is_stable() {
    assert_eq!(GLOBAL_NAMESPACE, "global");
}

#[test]
fn memory_category_display_outputs_expected_values() {
    assert_eq!(MemoryCategory::Core.to_string(), "core");
    assert_eq!(MemoryCategory::Daily.to_string(), "daily");
    assert_eq!(MemoryCategory::Conversation.to_string(), "conversation");
    assert_eq!(
        MemoryCategory::Custom("project_notes".into()).to_string(),
        "custom:project_notes"
    );
}

#[test]
fn memory_category_serde_uses_snake_case() {
    assert_eq!(
        serde_json::to_string(&MemoryCategory::Core).unwrap(),
        "\"core\""
    );
    assert_eq!(
        serde_json::to_string(&MemoryCategory::Daily).unwrap(),
        "\"daily\""
    );
    assert_eq!(
        serde_json::to_string(&MemoryCategory::Conversation).unwrap(),
        "\"conversation\""
    );
    assert_eq!(
        serde_json::to_string(&MemoryCategory::Custom("core".into())).unwrap(),
        "\"custom:core\""
    );
    for category in [
        MemoryCategory::Core,
        MemoryCategory::Daily,
        MemoryCategory::Conversation,
        MemoryCategory::Custom("core".into()),
        MemoryCategory::Custom("tool_memory".into()),
    ] {
        assert_eq!(
            category.to_string().parse::<MemoryCategory>().unwrap(),
            category
        );
        let json = serde_json::to_string(&category).unwrap();
        assert_eq!(
            serde_json::from_str::<MemoryCategory>(&json).unwrap(),
            category
        );
    }
    assert!("project_notes".parse::<MemoryCategory>().is_err());
}

#[test]
fn memory_entry_roundtrip_preserves_optional_fields() {
    let entry = MemoryEntry {
        id: "id-1".into(),
        key: "favorite_language".into(),
        content: "Rust".into(),
        namespace: Some("global".into()),
        category: MemoryCategory::Core,
        timestamp: "2026-02-16T00:00:00Z".into(),
        session_id: Some("session-abc".into()),
        score: Some(0.98),
        taint: MemoryTaint::Internal,
    };
    let json = serde_json::to_string(&entry).unwrap();
    let parsed: MemoryEntry = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.id, "id-1");
    assert_eq!(parsed.namespace.as_deref(), Some("global"));
    assert_eq!(parsed.category, MemoryCategory::Core);
    assert_eq!(parsed.session_id.as_deref(), Some("session-abc"));
    assert_eq!(parsed.score, Some(0.98));
    assert_eq!(parsed.taint, MemoryTaint::Internal);
}

#[test]
fn memory_taint_defaults_to_internal_for_legacy_rows() {
    let legacy = r#"{
        "id":"x","key":"k","content":"c","namespace":null,
        "category":"core","timestamp":"2026-01-01T00:00:00Z",
        "session_id":null,"score":null
    }"#;
    let parsed: MemoryEntry = serde_json::from_str(legacy).unwrap();
    assert_eq!(parsed.taint, MemoryTaint::Internal);
}

#[test]
fn memory_taint_db_str_roundtrip_and_fails_closed() {
    assert_eq!(MemoryTaint::Internal.as_db_str(), "internal");
    assert_eq!(MemoryTaint::ExternalSync.as_db_str(), "external_sync");
    assert_eq!(MemoryTaint::from_db_str("internal"), MemoryTaint::Internal);
    assert_eq!(
        MemoryTaint::from_db_str("external_sync"),
        MemoryTaint::ExternalSync
    );
    // Unknown / corrupt values fail closed to the restrictive variant.
    assert_eq!(MemoryTaint::from_db_str(""), MemoryTaint::ExternalSync);
    assert_eq!(
        MemoryTaint::from_db_str("EXTERNAL_SYNC"),
        MemoryTaint::ExternalSync
    );
    assert_eq!(
        MemoryTaint::from_db_str("future"),
        MemoryTaint::ExternalSync
    );
}

#[test]
fn memory_taint_serde_unknown_values_fail_closed() {
    assert_eq!(
        serde_json::from_str::<MemoryTaint>("\"unexpected\"").unwrap(),
        MemoryTaint::ExternalSync
    );
    assert_eq!(
        serde_json::to_string(&MemoryTaint::ExternalSync).unwrap(),
        "\"external_sync\""
    );
}

#[test]
fn memory_item_kind_serde_uses_snake_case() {
    assert_eq!(
        serde_json::to_string(&MemoryItemKind::Document).unwrap(),
        "\"document\""
    );
    let decoded: MemoryItemKind = serde_json::from_str("\"episodic\"").unwrap();
    assert_eq!(decoded, MemoryItemKind::Episodic);
}

#[test]
fn namespace_document_input_defaults_optional_fields() {
    let value = json!({
        "namespace": "global", "key": "note-1", "title": "Title",
        "content": "Body", "source_type": "manual", "priority": "normal",
        "metadata": {}, "category": "core"
    });
    let parsed: NamespaceDocumentInput = serde_json::from_value(value).unwrap();
    assert!(parsed.tags.is_empty());
    assert!(parsed.session_id.is_none());
    assert!(parsed.document_id.is_none());
    assert_eq!(parsed.taint, MemoryTaint::Internal);
}

#[test]
fn namespace_document_input_taint_roundtrips_external_sync() {
    let input = NamespaceDocumentInput {
        namespace: "skill-gmail".into(),
        key: "thread-1".into(),
        title: "Subject".into(),
        content: "Body".into(),
        source_type: "composio-sync".into(),
        priority: "medium".into(),
        tags: Vec::new(),
        metadata: json!({}),
        category: "core".into(),
        session_id: None,
        document_id: None,
        taint: MemoryTaint::ExternalSync,
    };
    let value = serde_json::to_value(&input).unwrap();
    assert_eq!(
        value.get("taint").and_then(|v| v.as_str()),
        Some("external_sync")
    );
    let parsed: NamespaceDocumentInput = serde_json::from_value(value).unwrap();
    assert_eq!(parsed.taint, MemoryTaint::ExternalSync);
}

#[test]
fn retrieval_score_breakdown_default_is_zeroed() {
    let b = RetrievalScoreBreakdown::default();
    assert_eq!(b.keyword_relevance, 0.0);
    assert_eq!(b.vector_similarity, 0.0);
    assert_eq!(b.graph_relevance, 0.0);
    assert_eq!(b.episodic_relevance, 0.0);
    assert_eq!(b.freshness, 0.0);
    assert_eq!(b.final_score, 0.0);
}

#[test]
fn memory_kv_record_roundtrips_with_optional_namespace() {
    for record in [
        MemoryKvRecord {
            namespace: None,
            key: "theme".into(),
            value: json!("dark"),
            updated_at: 1.5,
        },
        MemoryKvRecord {
            namespace: Some("project".into()),
            key: "state".into(),
            value: json!({"open": true}),
            updated_at: 2.5,
        },
    ] {
        let value = serde_json::to_value(&record).unwrap();
        let decoded: MemoryKvRecord = serde_json::from_value(value).unwrap();
        assert_eq!(decoded.namespace, record.namespace);
        assert_eq!(decoded.key, record.key);
        assert_eq!(decoded.value, record.value);
        assert_eq!(decoded.updated_at, record.updated_at);
    }
}

#[test]
fn namespace_memory_hit_defaults_optional_fields_and_taint() {
    let hit: NamespaceMemoryHit = serde_json::from_value(json!({
        "id": "hit-1", "kind": "document", "namespace": "global",
        "key": "note-1", "title": "Title", "content": "Body",
        "category": "core", "source_type": "manual", "updated_at": 3.5,
        "score": 0.8,
        "score_breakdown": {
            "keyword_relevance": 0.5, "vector_similarity": 0.2,
            "graph_relevance": 0.0, "episodic_relevance": 0.0,
            "freshness": 0.1, "final_score": 0.8
        }
    }))
    .unwrap();
    assert!(hit.document_id.is_none());
    assert!(hit.chunk_id.is_none());
    assert!(hit.supporting_relations.is_empty());
    assert_eq!(hit.kind, MemoryItemKind::Document);
    assert_eq!(hit.taint, MemoryTaint::Internal);
}
