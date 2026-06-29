//! Tests for the SQLite-backed graph occurrence adapter.

use crate::memory::graph::EntityOccurrenceIndex;
use crate::memory::retrieval::graph_adapter::ConfigEntityIndex;
use crate::memory::retrieval::test_support::{fixed_ts, index_entity_occurrence, test_config};
use crate::memory::score::extract::EntityKind;

#[test]
fn config_entity_index_reads_nodes_and_entities_from_sqlite_index() {
    let (_tmp, cfg) = test_config();
    let ts = fixed_ts().timestamp_millis();
    index_entity_occurrence(
        &cfg,
        "topic:phoenix",
        EntityKind::Topic,
        "Phoenix",
        "node-a",
        "summary",
        ts,
        Some("tree:eng"),
    );
    index_entity_occurrence(
        &cfg,
        "person:alice",
        EntityKind::Person,
        "Alice",
        "node-a",
        "summary",
        ts,
        Some("tree:eng"),
    );
    index_entity_occurrence(
        &cfg,
        "topic:phoenix",
        EntityKind::Topic,
        "Phoenix",
        "node-b",
        "leaf",
        ts + 1,
        None,
    );

    let idx = ConfigEntityIndex::new(&cfg);
    let nodes = idx.nodes_for_entity("topic:phoenix").unwrap();
    assert_eq!(nodes, vec!["node-b".to_string(), "node-a".to_string()]);

    let entities = idx.entities_on_node("node-a").unwrap();
    assert_eq!(
        entities,
        vec!["person:alice".to_string(), "topic:phoenix".to_string()]
    );
    assert!(idx.nodes_for_entity("topic:missing").unwrap().is_empty());
    assert!(idx.entities_on_node("node-missing").unwrap().is_empty());
}
