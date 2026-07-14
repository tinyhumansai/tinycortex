use super::*;

fn config() -> (tempfile::TempDir, MemoryConfig) {
    let temp = tempfile::tempdir().unwrap();
    let config = MemoryConfig::new(temp.path());
    (temp, config)
}

fn insert_entity(config: &MemoryConfig, entity: &str, node: &str, timestamp_ms: i64, score: f64) {
    with_connection(config, |connection| {
        connection.execute(
            "INSERT INTO mem_tree_entity_index
             (entity_id, node_id, node_kind, entity_kind, surface, score, timestamp_ms, tree_id, is_user)
             VALUES (?1, ?2, 'leaf', 'topic', ?1, ?3, ?4, NULL, 0)",
            rusqlite::params![entity, node, score, timestamp_ms],
        )?;
        Ok(())
    })
    .unwrap();
}

#[test]
fn lookup_entity_in_window_filters_orders_and_limits() {
    let (_temp, config) = config();
    insert_entity(&config, "topic:x", "old", 10, 0.2);
    insert_entity(&config, "topic:x", "mid", 20, 0.4);
    insert_entity(&config, "topic:x", "new", 30, 0.6);
    insert_entity(&config, "topic:y", "other", 25, 0.9);

    let hits = lookup_entity_in_window(&config, "topic:x", Some(15), Some(35), Some(1)).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].node_id, "new");
}

#[test]
fn list_entity_ids_for_node_is_distinct_and_deterministic() {
    let (_temp, config) = config();
    insert_entity(&config, "topic:a", "node", 10, 0.4);
    insert_entity(&config, "topic:b", "node", 20, 0.9);

    assert_eq!(
        list_entity_ids_for_node(&config, "node").unwrap(),
        vec!["topic:b", "topic:a"]
    );
}
