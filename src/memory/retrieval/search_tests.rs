//! `search_entities` tests. Ported from OpenHuman's
//! `memory_tree::retrieval::search`, reseeding the entity index directly
//! instead of running the full ingest pipeline.

use super::*;
use crate::memory::retrieval::test_support::{index_entity_occurrence, test_config};
use crate::memory::score::extract::EntityKind;

/// Seed a handful of entity-index rows mimicking what ingest would produce for
/// one message mentioning two emails and a hashtag.
fn seed_entities(cfg: &crate::memory::config::MemoryConfig, node_id: &str) {
    index_entity_occurrence(
        cfg,
        "email:alice@example.com",
        EntityKind::Email,
        "alice@example.com",
        node_id,
        "leaf",
        1_700_000_000_000,
        None,
    );
    index_entity_occurrence(
        cfg,
        "email:bob@example.com",
        EntityKind::Email,
        "bob@example.com",
        node_id,
        "leaf",
        1_700_000_000_000,
        None,
    );
    index_entity_occurrence(
        cfg,
        "hashtag:launch-q2",
        EntityKind::Hashtag,
        "#launch-q2",
        node_id,
        "leaf",
        1_700_000_000_000,
        None,
    );
}

#[test]
fn empty_index_returns_empty_vec() {
    let (_tmp, cfg) = test_config();
    let matches = search_entities(&cfg, "alice", None, 10).unwrap();
    assert!(matches.is_empty());
}

#[test]
fn blank_query_returns_empty() {
    let (_tmp, cfg) = test_config();
    seed_entities(&cfg, "leaf-1");
    let matches = search_entities(&cfg, "   ", None, 10).unwrap();
    assert!(matches.is_empty());
}

#[test]
fn matches_on_entity_id_substring() {
    let (_tmp, cfg) = test_config();
    seed_entities(&cfg, "leaf-1");
    let matches = search_entities(&cfg, "alice", None, 10).unwrap();
    assert!(
        matches
            .iter()
            .any(|m| m.canonical_id == "email:alice@example.com"),
        "expected alice's canonical id; got {matches:?}"
    );
}

#[test]
fn matches_on_surface_substring() {
    let (_tmp, cfg) = test_config();
    seed_entities(&cfg, "leaf-1");
    // "example.com" appears in the surface form and the canonical id.
    let matches = search_entities(&cfg, "example.com", None, 10).unwrap();
    assert!(
        matches.iter().any(|m| m.canonical_id.contains("alice")),
        "surface-matched row must surface; got {matches:?}"
    );
}

#[test]
fn like_metacharacters_are_matched_literally() {
    let (_tmp, cfg) = test_config();
    index_entity_occurrence(
        &cfg,
        "topic:100%_ready",
        EntityKind::Topic,
        "100%_ready",
        "leaf-literal",
        "leaf",
        1_700_000_000_000,
        None,
    );
    index_entity_occurrence(
        &cfg,
        "topic:100x-ready",
        EntityKind::Topic,
        "100x-ready",
        "leaf-wildcard-decoy",
        "leaf",
        1_700_000_000_001,
        None,
    );

    let matches = search_entities(&cfg, "100%_", None, 10).unwrap();
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].canonical_id, "topic:100%_ready");
}

#[test]
fn escape_like_literal_escapes_escape_and_wildcard_characters() {
    assert_eq!(escape_like_literal(r"a\b%c_d"), r"a\\b\%c\_d");
}

#[test]
fn kind_filter_narrows_results() {
    let (_tmp, cfg) = test_config();
    seed_entities(&cfg, "leaf-1");
    let only_hashtags = search_entities(&cfg, "launch", Some(&[EntityKind::Hashtag]), 10).unwrap();
    assert!(only_hashtags
        .iter()
        .all(|m| matches!(m.kind, EntityKind::Hashtag)));
    assert!(!only_hashtags.is_empty());
}

#[test]
fn matches_aggregate_across_multiple_nodes() {
    let (_tmp, cfg) = test_config();
    // Same entity mentioned on two distinct nodes → mention_count >= 2.
    index_entity_occurrence(
        &cfg,
        "email:alice@example.com",
        EntityKind::Email,
        "alice@example.com",
        "leaf-a",
        "leaf",
        1_700_000_000_000,
        None,
    );
    index_entity_occurrence(
        &cfg,
        "email:alice@example.com",
        EntityKind::Email,
        "alice@example.com",
        "leaf-b",
        "leaf",
        1_700_000_000_001,
        None,
    );
    let matches = search_entities(&cfg, "alice", None, 10).unwrap();
    let alice = matches
        .iter()
        .find(|m| m.canonical_id == "email:alice@example.com")
        .expect("alice should be in matches");
    assert!(
        alice.mention_count >= 2,
        "expected >= 2 aggregated mentions, got {}",
        alice.mention_count
    );
}

#[test]
fn limit_truncates_results() {
    let (_tmp, cfg) = test_config();
    for (i, local) in ["alice", "bob", "charlie", "dana", "eric"]
        .iter()
        .enumerate()
    {
        index_entity_occurrence(
            &cfg,
            &format!("email:{local}@example.com"),
            EntityKind::Email,
            &format!("{local}@example.com"),
            &format!("leaf-{i}"),
            "leaf",
            1_700_000_000_000,
            None,
        );
    }
    let matches = search_entities(&cfg, "example.com", None, 2).unwrap();
    assert!(matches.len() <= 2);
}

#[test]
fn build_sql_without_kinds_has_no_in_clause() {
    let (sql, _params) = build_sql_and_params("%a%", None, 5);
    assert!(sql.contains("LOWER(entity_id) LIKE"));
    assert!(sql.contains("ESCAPE '\\'"));
    assert!(!sql.contains("entity_kind IN"));
}

#[test]
fn build_sql_with_kinds_adds_in_clause() {
    let kinds = [EntityKind::Email, EntityKind::Hashtag];
    let (sql, params) = build_sql_and_params("%x%", Some(&kinds), 5);
    assert!(sql.contains("entity_kind IN"));
    // pattern + 2 kinds + limit = 4 params
    assert_eq!(params.len(), 4);
}

#[test]
fn zero_limit_defaults_to_five() {
    let config = MemoryConfig::new("/tmp/search-limit-test");
    assert_eq!(
        normalise_limit(&config, 0),
        config.retrieval.limits.search_default_limit
    );
}

#[test]
fn huge_limit_is_clamped() {
    let config = MemoryConfig::new("/tmp/search-limit-test");
    assert_eq!(
        normalise_limit(&config, 10_000),
        config.retrieval.limits.max_limit
    );
}
