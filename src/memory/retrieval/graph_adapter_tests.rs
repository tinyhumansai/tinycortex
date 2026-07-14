//! Tests for the SQLite-backed graph occurrence adapter.

use crate::memory::graph::co_occurring_entities;
use crate::memory::graph::EntityOccurrenceIndex;
use crate::memory::retrieval::graph_adapter::{ConfigEntityIndex, OCCURRENCE_LOOKUP_LIMIT};
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

#[test]
fn sqlite_cooccurrence_fast_path_counts_beyond_occurrence_cap_and_filters_dropped_nodes() {
    use crate::memory::chunks::with_connection;

    let (_tmp, cfg) = test_config();
    with_connection(&cfg, |conn| {
        let tx = conn.unchecked_transaction()?;
        for index in 0..=OCCURRENCE_LOOKUP_LIMIT {
            let node = format!("node-{index}");
            for entity in ["topic:phoenix", "person:alice"] {
                tx.execute(
                    "INSERT INTO mem_tree_entity_index
                     (entity_id,node_id,node_kind,entity_kind,surface,score,timestamp_ms,tree_id,is_user)
                     VALUES (?1,?2,'leaf','topic',?1,1.0,?3,NULL,0)",
                    rusqlite::params![entity, node, index as i64],
                )?;
            }
        }
        tx.execute(
            "INSERT INTO mem_tree_score
             (chunk_id,total,token_count_signal,unique_words_signal,metadata_weight,
              source_weight,interaction_weight,entity_density,dropped,computed_at_ms)
             VALUES ('node-0',0,0,0,0,0,0,0,1,0)",
            [],
        )?;
        tx.commit()?;
        Ok(())
    })
    .unwrap();

    let edges =
        co_occurring_entities(&ConfigEntityIndex::new(&cfg), "topic:phoenix", Some(10)).unwrap();

    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].object, "person:alice");
    assert_eq!(edges[0].weight, OCCURRENCE_LOOKUP_LIMIT as u32);
}
