use super::*;

#[test]
fn relation_rule_normalizes_supported_predicates() {
    let owns = relation_rule("works_on").unwrap();
    assert_eq!(owns.canonical, "OWNS");
    assert_eq!(owns.allowed_head, PERSON_TYPES);

    let prefers = relation_rule("prefers").unwrap();
    assert_eq!(prefers.canonical, "PREFERS");

    let deadline = relation_rule("due_on").unwrap();
    assert_eq!(deadline.canonical, "HAS_DEADLINE");
}

#[test]
fn type_allowed_honors_allowlist_and_empty_list() {
    assert!(type_allowed("PERSON", PERSON_TYPES));
    assert!(!type_allowed("PROJECT", PERSON_TYPES));
    assert!(type_allowed("ANYTHING", &[]));
}

#[test]
fn resolve_person_alias_uses_known_people_map() {
    let mut known = std::collections::HashMap::new();
    known.insert("ALICE".to_string(), "ALICE SMITH".to_string());
    assert_eq!(resolve_person_alias("ALICE", &known), "ALICE SMITH");
    assert_eq!(resolve_person_alias("BOB", &known), "BOB");
}

#[test]
fn add_entity_tracks_highest_confidence_and_person_aliases() {
    let mut acc = ExtractionAccumulator::default();
    let first = acc.add_entity("Alice Smith", "PERSON", 0.6).unwrap();
    let second = acc.add_entity("Alice Smith", "PERSON", 0.9).unwrap();
    assert_eq!(first, "ALICE SMITH");
    assert_eq!(second, "ALICE SMITH");
    assert_eq!(acc.entities["ALICE SMITH"].confidence, 0.9);
    assert_eq!(
        acc.known_people.get("ALICE").map(String::as_str),
        Some("ALICE SMITH")
    );
}

#[test]
fn add_relation_rejects_invalid_or_self_relations() {
    let mut acc = ExtractionAccumulator::default();
    acc.add_relation(
        "Alice",
        "PERSON",
        "owns",
        "Alice",
        "PERSON",
        0.8,
        0,
        0,
        Map::new(),
    );
    assert!(acc.relations.is_empty(), "self relation should be dropped");

    acc.add_relation(
        "Alice",
        "PERSON",
        "unknown_predicate",
        "Project X",
        "PROJECT",
        0.8,
        0,
        0,
        Map::new(),
    );
    assert!(
        acc.relations.is_empty(),
        "unknown predicate should be ignored"
    );
}

#[test]
fn add_relation_canonicalizes_predicate_and_collects_chunk_index() {
    let mut acc = ExtractionAccumulator::default();
    acc.add_relation(
        "Alice",
        "PERSON",
        "works_on",
        "Phoenix",
        "PROJECT",
        0.8,
        3,
        11,
        Map::new(),
    );
    assert_eq!(acc.relations.len(), 1);
    let relation = &acc.relations[0];
    assert_eq!(relation.predicate, "OWNS");
    assert_eq!(relation.subject, "ALICE");
    assert_eq!(relation.object, "PHOENIX");
    assert!(relation.chunk_indexes.contains(&3));
    assert_eq!(relation.order_index, 11);
}
