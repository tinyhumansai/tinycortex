use super::*;
use crate::memory::store::entity_index::types::{CanonicalEntity, EntityKind};

fn index() -> EntityIndex {
    EntityIndex::open_in_memory().unwrap()
}

fn sample_entity(id: &str) -> CanonicalEntity {
    CanonicalEntity {
        canonical_id: format!("email:{id}"),
        kind: EntityKind::Email,
        surface: format!("{id}@example.com"),
        span_start: 0,
        span_end: (id.len() + 12) as u32,
        score: 1.0,
    }
}

#[test]
fn index_and_lookup_entity() {
    let idx = index();
    let e = sample_entity("alice");
    idx.index_entity(&e, "chunk-1", "leaf", 1000, Some("source:chat"))
        .unwrap();
    idx.index_entity(&e, "chunk-2", "leaf", 2000, Some("source:chat"))
        .unwrap();

    let hits = idx.lookup_entity("email:alice", None).unwrap();
    assert_eq!(hits.len(), 2);
    // newest first
    assert_eq!(hits[0].node_id, "chunk-2");
    assert_eq!(hits[1].node_id, "chunk-1");
}

#[test]
fn index_batch() {
    let idx = index();
    let entities = vec![sample_entity("a"), sample_entity("b"), sample_entity("c")];
    let n = idx
        .index_entities(&entities, "chunk-1", "leaf", 1000, None)
        .unwrap();
    assert_eq!(n, 3);
    assert_eq!(idx.count_entity_index().unwrap(), 3);
}

#[test]
fn clear_entity_index_drops_stale_rows() {
    let idx = index();
    let a = sample_entity("a");
    let b = sample_entity("b");
    idx.index_entities(&[a.clone(), b], "chunk-1", "leaf", 1000, None)
        .unwrap();
    assert_eq!(idx.count_entity_index().unwrap(), 2);

    // Simulate a re-score that only keeps entity "a".
    let cleared = idx.clear_entity_index_for_node("chunk-1").unwrap();
    assert_eq!(cleared, 2);
    idx.index_entities(&[a], "chunk-1", "leaf", 1000, None)
        .unwrap();

    let hits = idx.lookup_entity("email:b", None).unwrap();
    assert!(hits.is_empty(), "stale entity should be removed");
    let hits = idx.lookup_entity("email:a", None).unwrap();
    assert_eq!(hits.len(), 1);
}

#[test]
fn index_idempotent_per_entity_node_pair() {
    let idx = index();
    let e = sample_entity("alice");
    idx.index_entity(&e, "chunk-1", "leaf", 1000, None).unwrap();
    idx.index_entity(&e, "chunk-1", "leaf", 1000, None).unwrap();
    assert_eq!(idx.count_entity_index().unwrap(), 1);
}

#[test]
fn lookup_limit_respected() {
    let idx = index();
    let e = sample_entity("alice");
    for i in 0..5 {
        idx.index_entity(&e, &format!("chunk-{i}"), "leaf", 1000 + i as i64, None)
            .unwrap();
    }
    let hits = idx.lookup_entity("email:alice", Some(2)).unwrap();
    assert_eq!(hits.len(), 2);
}

/// `index_summary_entity_ids` must write a parseable `entity_kind` (the
/// "<kind>" prefix before `:`) so `lookup_entity` can still round-trip rows
/// through `EntityKind::parse`.
#[test]
fn summary_entity_index_kind_is_parseable() {
    let idx = index();

    // Seed a leaf hit so lookup_entity has something leafy to mix with.
    let leaf_entity = sample_entity("alice");
    idx.index_entity(&leaf_entity, "leaf-1", "leaf", 1000, Some("tree-1"))
        .unwrap();

    let n = idx
        .index_summary_entity_ids(
            &["email:alice@example.com".into(), "hashtag:launch-q2".into()],
            "summary-1",
            0.84,
            2000,
            Some("tree-1"),
        )
        .unwrap();
    assert_eq!(n, 2);

    let hits = idx.lookup_entity("email:alice@example.com", None).unwrap();
    assert_eq!(hits.len(), 1, "summary row should be discoverable");
    assert_eq!(hits[0].node_id, "summary-1");
    assert_eq!(hits[0].node_kind, "summary");
    assert_eq!(hits[0].entity_kind, EntityKind::Email);

    let hits = idx.lookup_entity("hashtag:launch-q2", None).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].entity_kind, EntityKind::Hashtag);

    let hits = idx.lookup_entity("email:alice", None).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].entity_kind, EntityKind::Email);
}

#[test]
fn list_entity_ids_for_node_orders_by_score() {
    let idx = index();
    let mut high = sample_entity("high");
    high.score = 0.9;
    let mut low = sample_entity("low");
    low.score = 0.1;
    idx.index_entities(&[low, high], "chunk-1", "leaf", 1000, None)
        .unwrap();
    let ids = idx.list_entity_ids_for_node("chunk-1").unwrap();
    assert_eq!(ids, vec!["email:high".to_string(), "email:low".to_string()]);
}

#[test]
fn default_identity_marks_no_rows_as_user() {
    let idx = index();
    let e = sample_entity("alice");
    idx.index_entity(&e, "chunk-1", "leaf", 1000, None).unwrap();
    let hits = idx.lookup_entity("email:alice", None).unwrap();
    assert!(!hits[0].is_user);
}

/// A custom [`SelfIdentity`] flips `is_user` for matching surfaces. This pins
/// the identity abstraction the storage primitive exposes to hosts.
#[test]
fn custom_identity_marks_self_rows() {
    struct OnlyAlice;
    impl SelfIdentity for OnlyAlice {
        fn is_self(&self, kind: EntityKind, surface: &str) -> bool {
            kind == EntityKind::Email && surface == "alice@example.com"
        }
    }

    let idx = EntityIndex::open_in_memory_with_identity(std::sync::Arc::new(OnlyAlice)).unwrap();
    idx.index_entity(&sample_entity("alice"), "c1", "leaf", 1000, None)
        .unwrap();
    idx.index_entity(&sample_entity("bob"), "c2", "leaf", 1000, None)
        .unwrap();

    assert!(idx.lookup_entity("email:alice", None).unwrap()[0].is_user);
    assert!(!idx.lookup_entity("email:bob", None).unwrap()[0].is_user);

    // Summary path resolves identity from the canonical id too.
    idx.index_summary_entity_ids(&["email:alice@example.com".into()], "s1", 0.5, 3000, None)
        .unwrap();
    assert!(
        idx.lookup_entity("email:alice@example.com", None).unwrap()[0].is_user,
        "summary self-identity should resolve from canonical id"
    );
}
