use super::*;
use crate::memory::score::extract::ExtractedEntity;

fn entity(kind: EntityKind, text: &str) -> ExtractedEntity {
    ExtractedEntity {
        kind,
        text: text.to_string(),
        span_start: 0,
        span_end: text.chars().count() as u32,
        score: 1.0,
    }
}

#[test]
fn email_case_insensitive_canonicalises() {
    let a = canonical_id_for(EntityKind::Email, "Alice@Example.com");
    let b = canonical_id_for(EntityKind::Email, "alice@example.com");
    assert_eq!(a, b);
    assert_eq!(a, "email:alice@example.com");
}

#[test]
fn handle_strips_leading_at() {
    let a = canonical_id_for(EntityKind::Handle, "@alice");
    let b = canonical_id_for(EntityKind::Handle, "alice");
    assert_eq!(a, b);
    assert_eq!(a, "handle:alice");
}

#[test]
fn hashtag_strips_leading_hash() {
    let a = canonical_id_for(EntityKind::Hashtag, "#launch");
    let b = canonical_id_for(EntityKind::Hashtag, "launch");
    assert_eq!(a, b);
}

#[test]
fn url_preserves_case() {
    let id = canonical_id_for(EntityKind::Url, " https://example.com/Path?Token=ABC ");
    assert_eq!(id, "url:https://example.com/Path?Token=ABC");
}

#[test]
fn canonicalise_batch_preserves_spans() {
    let ex = ExtractedEntities {
        entities: vec![
            entity(EntityKind::Email, "Alice@Example.com"),
            entity(EntityKind::Email, "alice@example.com"),
        ],
        topics: vec![],
        llm_importance: None,
        llm_importance_reason: None,
    };
    let out = canonicalise(&ex);
    assert_eq!(out.len(), 2);
    // Both map to the same canonical id (merge-equivalent)
    assert_eq!(out[0].canonical_id, out[1].canonical_id);
    // But surface forms remain distinct
    assert_ne!(out[0].surface, out[1].surface);
}

#[test]
fn different_kinds_produce_different_ids_for_same_text() {
    assert_ne!(
        canonical_id_for(EntityKind::Handle, "alice"),
        canonical_id_for(EntityKind::Person, "alice")
    );
}

// ── Topic canonicalisation (topic-tree scope) ────

use crate::memory::score::extract::ExtractedTopic;

fn topic(label: &str, score: f32) -> ExtractedTopic {
    ExtractedTopic {
        label: label.to_string(),
        score,
    }
}

#[test]
fn topics_are_promoted_to_canonical_entities() {
    let ex = ExtractedEntities {
        entities: vec![],
        topics: vec![topic("phoenix", 0.72), topic("migration", 0.60)],
        llm_importance: None,
        llm_importance_reason: None,
    };
    let out = canonicalise(&ex);
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].kind, EntityKind::Topic);
    assert_eq!(out[0].canonical_id, "topic:phoenix");
    assert!((out[0].score - 0.72).abs() < 1e-6);
    assert_eq!(out[1].canonical_id, "topic:migration");
}

#[test]
fn topic_canonicalisation_lowercases() {
    let ex = ExtractedEntities {
        entities: vec![],
        topics: vec![topic("Phoenix", 1.0), topic("PHOENIX", 0.5)],
        llm_importance: None,
        llm_importance_reason: None,
    };
    let out = canonicalise(&ex);
    // Both normalise to "topic:phoenix" — second occurrence is deduped.
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].canonical_id, "topic:phoenix");
    // First-seen surface is preserved.
    assert_eq!(out[0].surface, "Phoenix");
}

#[test]
fn hashtag_and_topic_with_same_label_coexist() {
    // "#launch" regex → EntityKind::Hashtag, LLM theme "launch" → Topic.
    // They stay as two distinct canonical entities — different kind,
    // different canonical_id prefix.
    let ex = ExtractedEntities {
        entities: vec![ExtractedEntity {
            kind: EntityKind::Hashtag,
            text: "launch".into(),
            span_start: 0,
            span_end: 6,
            score: 1.0,
        }],
        topics: vec![topic("launch", 0.8)],
        llm_importance: None,
        llm_importance_reason: None,
    };
    let out = canonicalise(&ex);
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].kind, EntityKind::Hashtag);
    assert_eq!(out[0].canonical_id, "hashtag:launch");
    assert_eq!(out[1].kind, EntityKind::Topic);
    assert_eq!(out[1].canonical_id, "topic:launch");
}

#[test]
fn canonicalise_mixes_entities_and_topics_in_order() {
    // Entities come first, topics appended after.
    let ex = ExtractedEntities {
        entities: vec![entity(EntityKind::Email, "alice@example.com")],
        topics: vec![topic("phoenix", 0.7)],
        llm_importance: None,
        llm_importance_reason: None,
    };
    let out = canonicalise(&ex);
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].kind, EntityKind::Email);
    assert_eq!(out[1].kind, EntityKind::Topic);
}

#[test]
fn topic_entity_kind_round_trips_through_parse() {
    // Ensure the Topic variant survives the round-trip used by the entity index.
    assert_eq!(EntityKind::parse("topic"), Ok(EntityKind::Topic));
    assert_eq!(EntityKind::Topic.as_str(), "topic");
}
