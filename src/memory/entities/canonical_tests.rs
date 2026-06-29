//! Tests for canonical id derivation and slugging. Ported from OpenHuman's
//! `memory_tree::score::resolver` (canonical-id cases) and `memory_entities`
//! (slug cases).

use super::*;

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
    assert_eq!(a, "hashtag:launch");
}

#[test]
fn topic_lowercases_and_strips_prefix() {
    let a = canonical_id_for(EntityKind::Topic, "#Phoenix");
    let b = canonical_id_for(EntityKind::Topic, "phoenix");
    assert_eq!(a, b);
    assert_eq!(a, "topic:phoenix");
}

#[test]
fn url_preserves_case() {
    let id = canonical_id_for(EntityKind::Url, " https://example.com/Path?Token=ABC ");
    assert_eq!(id, "url:https://example.com/Path?Token=ABC");
}

#[test]
fn different_kinds_produce_different_ids_for_same_text() {
    assert_ne!(
        canonical_id_for(EntityKind::Handle, "alice"),
        canonical_id_for(EntityKind::Person, "alice")
    );
}

#[test]
fn hashtag_and_topic_with_same_label_stay_distinct() {
    assert_eq!(
        canonical_id_for(EntityKind::Hashtag, "launch"),
        "hashtag:launch"
    );
    assert_eq!(
        canonical_id_for(EntityKind::Topic, "launch"),
        "topic:launch"
    );
}

#[test]
fn slugify_strips_filesystem_unsafe_chars() {
    assert_eq!(slugify_id("person:alice"), "person_alice");
    assert_eq!(
        slugify_id("url:https://x.com/path"),
        "url_https___x.com_path"
    );
}
