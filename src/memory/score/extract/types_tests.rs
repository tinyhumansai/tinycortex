use super::*;

#[test]
fn entity_kind_round_trip() {
    for k in [
        EntityKind::Email,
        EntityKind::Url,
        EntityKind::Handle,
        EntityKind::Hashtag,
        EntityKind::Person,
        EntityKind::Organization,
        EntityKind::Location,
        EntityKind::Event,
        EntityKind::Product,
        EntityKind::Datetime,
        EntityKind::Technology,
        EntityKind::Artifact,
        EntityKind::Quantity,
        EntityKind::Misc,
        EntityKind::Topic,
    ] {
        assert_eq!(EntityKind::parse(k.as_str()).unwrap(), k);
    }
}

#[test]
fn mechanical_classification() {
    assert!(EntityKind::Email.is_mechanical());
    assert!(EntityKind::Url.is_mechanical());
    assert!(EntityKind::Handle.is_mechanical());
    assert!(EntityKind::Hashtag.is_mechanical());
    assert!(!EntityKind::Person.is_mechanical());
}

#[test]
fn unique_entity_count_dedups_case_insensitive() {
    let e = ExtractedEntities {
        entities: vec![
            ExtractedEntity {
                kind: EntityKind::Person,
                text: "Alice".into(),
                span_start: 0,
                span_end: 5,
                score: 1.0,
            },
            ExtractedEntity {
                kind: EntityKind::Person,
                text: "alice".into(),
                span_start: 10,
                span_end: 15,
                score: 1.0,
            },
        ],
        topics: vec![],
        llm_importance: None,
        llm_importance_reason: None,
    };
    assert_eq!(e.unique_entity_count(), 1);
}

#[test]
fn unique_entity_count_keeps_different_kinds_distinct() {
    let e = ExtractedEntities {
        entities: vec![
            ExtractedEntity {
                kind: EntityKind::Handle,
                text: "alice".into(),
                span_start: 0,
                span_end: 5,
                score: 1.0,
            },
            ExtractedEntity {
                kind: EntityKind::Hashtag,
                text: "alice".into(),
                span_start: 10,
                span_end: 15,
                score: 1.0,
            },
        ],
        topics: vec![],
        llm_importance: None,
        llm_importance_reason: None,
    };
    assert_eq!(e.unique_entity_count(), 2);
}

#[test]
fn merge_dedups_by_kind_text_span() {
    let mut a = ExtractedEntities {
        entities: vec![ExtractedEntity {
            kind: EntityKind::Email,
            text: "x@y.com".into(),
            span_start: 0,
            span_end: 7,
            score: 1.0,
        }],
        topics: vec![],
        llm_importance: None,
        llm_importance_reason: None,
    };
    let b = ExtractedEntities {
        entities: vec![
            ExtractedEntity {
                kind: EntityKind::Email,
                text: "x@y.com".into(),
                span_start: 0,
                span_end: 7,
                score: 1.0,
            }, // dup
            ExtractedEntity {
                kind: EntityKind::Email,
                text: "x@y.com".into(),
                span_start: 50,
                span_end: 57,
                score: 1.0,
            }, // different span — keep
        ],
        topics: vec![],
        llm_importance: None,
        llm_importance_reason: None,
    };
    a.merge(b);
    assert_eq!(a.entities.len(), 2);
}
