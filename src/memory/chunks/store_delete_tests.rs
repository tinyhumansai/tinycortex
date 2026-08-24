//! Unit tests for the deletes `super::store_delete` owns beyond the
//! source-scoped ones already covered in `store_tests`: the by-id delete and
//! the whole-tier purge.

use super::types::{chunk_id, Chunk, Metadata, SourceKind, SourceRef};
use super::{
    count_chunks, delete_chunk_by_id, get_chunk, is_source_ingested, purge_all, upsert_chunks,
    with_connection,
};
use crate::memory::config::MemoryConfig;
use chrono::{TimeZone, Utc};
use rusqlite::params;
use tempfile::TempDir;

fn test_config() -> (TempDir, MemoryConfig) {
    let tmp = TempDir::new().expect("tempdir");
    let cfg = MemoryConfig::new(tmp.path());
    (tmp, cfg)
}

fn chunk_with(source_id: &str, seq: u32, ts_ms: i64, content: &str) -> Chunk {
    let ts = Utc.timestamp_millis_opt(ts_ms).unwrap();
    Chunk {
        id: chunk_id(SourceKind::Chat, source_id, seq, content),
        content: content.to_string(),
        metadata: Metadata {
            source_kind: SourceKind::Chat,
            source_id: source_id.to_string(),
            owner: "alice@example.com".to_string(),
            timestamp: ts,
            time_range: (ts, ts),
            tags: vec!["eng".into()],
            source_ref: Some(SourceRef::new(format!("slack://{source_id}/{seq}"))),
            path_scope: None,
        },
        token_count: 12,
        seq_in_source: seq,
        created_at: ts,
        partial_message: false,
    }
}

fn count_rows(cfg: &MemoryConfig, table: &str) -> i64 {
    with_connection(cfg, |conn| {
        Ok(
            conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })?,
        )
    })
    .unwrap()
}

fn index_entity(cfg: &MemoryConfig, node_id: &str) {
    with_connection(cfg, |conn| {
        conn.execute(
            "INSERT INTO mem_tree_entity_index (
                entity_id, node_id, node_kind, entity_kind, surface, score, timestamp_ms
             ) VALUES ('entity:a', ?1, 'leaf', 'org', 'Acme', 0.9, 1700000000000)",
            params![node_id],
        )?;
        Ok(())
    })
    .unwrap();
}

#[test]
fn delete_chunk_by_id_takes_one_chunk_and_leaves_the_source() {
    let (_tmp, cfg) = test_config();
    let target = chunk_with("slack:#eng", 0, 2_000, "target");
    let sibling = chunk_with("slack:#eng", 1, 1_000, "sibling");
    upsert_chunks(&cfg, &[target.clone(), sibling.clone()]).unwrap();
    index_entity(&cfg, &target.id);
    index_entity(&cfg, &sibling.id);
    with_connection(&cfg, |conn| {
        conn.execute(
            "INSERT INTO mem_tree_ingested_sources (source_kind, source_id, ingested_at_ms)
             VALUES ('chat', 'slack:#eng', 1700000000000)",
            [],
        )?;
        Ok(())
    })
    .unwrap();

    assert_eq!(delete_chunk_by_id(&cfg, &target.id).unwrap(), 1);
    assert!(get_chunk(&cfg, &target.id).unwrap().is_none());
    assert!(get_chunk(&cfg, &sibling.id).unwrap().is_some());
    assert_eq!(count_rows(&cfg, "mem_tree_entity_index"), 1);
    assert!(
        is_source_ingested(&cfg, SourceKind::Chat, "slack:#eng").unwrap(),
        "a source with surviving chunks keeps its ingest gate"
    );

    // Idempotent, and an unknown id is not an error.
    assert_eq!(delete_chunk_by_id(&cfg, &target.id).unwrap(), 0);
    assert_eq!(delete_chunk_by_id(&cfg, "no-such-chunk").unwrap(), 0);
}

/// Seed one row in every table `purge_all` empties, so a table name that does
/// not exist in the schema fails the test rather than passing silently.
fn seed_full_tier(cfg: &MemoryConfig, chunk: &Chunk) {
    with_connection(cfg, |conn| {
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO mem_tree_score (
                chunk_id, total, token_count_signal, unique_words_signal, metadata_weight,
                source_weight, interaction_weight, entity_density, dropped, reason, computed_at_ms
             ) VALUES (?1, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 0, NULL, 1700000000000)",
            params![chunk.id],
        )?;
        tx.execute(
            "INSERT INTO mem_tree_entity_index (
                entity_id, node_id, node_kind, entity_kind, surface, score, timestamp_ms
             ) VALUES ('entity:a', ?1, 'leaf', 'org', 'Acme', 0.9, 1700000000000)",
            params![chunk.id],
        )?;
        tx.execute(
            "INSERT INTO mem_tree_entity_edges (entity_a, entity_b, weight, updated_ms)
             VALUES ('entity:a', 'entity:b', 1, 1700000000000)",
            [],
        )?;
        tx.execute(
            "INSERT INTO mem_tree_entity_hotness (entity_id, last_updated_ms)
             VALUES ('entity:a', 1700000000000)",
            [],
        )?;
        tx.execute(
            "INSERT INTO mem_tree_jobs (id, kind, payload_json, available_at_ms, created_at_ms)
             VALUES ('job-1', 'score', '{}', 1700000000000, 1700000000000)",
            [],
        )?;
        tx.execute(
            "INSERT INTO mem_tree_trees (id, kind, scope, max_level, created_at_ms)
             VALUES ('tree-1', 'source', 'slack:#eng', 0, 1700000000000)",
            [],
        )?;
        tx.execute(
            "INSERT INTO mem_tree_buffers (tree_id, level, updated_at_ms)
             VALUES ('tree-1', 0, 1700000000000)",
            [],
        )?;
        tx.execute(
            "INSERT INTO mem_tree_summaries (
                id, tree_id, tree_kind, level, content, token_count,
                time_range_start_ms, time_range_end_ms, sealed_at_ms
             ) VALUES ('sum-1', 'tree-1', 'source', 1, 'summary', 3,
                       1700000000000, 1700000000000, 1700000000000)",
            [],
        )?;
        tx.execute(
            "INSERT INTO mem_tree_summary_embeddings (
                summary_id, model_signature, vector, dim, created_at
             ) VALUES ('sum-1', 'test/model@3', ?1, 3, 1700000000.0)",
            params![vec![1_u8, 2, 3]],
        )?;
        tx.execute(
            "INSERT INTO mem_tree_summary_reembed_skipped (
                summary_id, model_signature, reason, skipped_at_ms
             ) VALUES ('sum-1', 'test/model@3', 'terminal', 1700000000000)",
            [],
        )?;
        tx.execute(
            "INSERT INTO mem_tree_chunk_embeddings (
                chunk_id, model_signature, vector, dim, created_at
             ) VALUES (?1, 'test/model@3', ?2, 3, 1700000000.0)",
            params![chunk.id, vec![1_u8, 2, 3]],
        )?;
        tx.execute(
            "INSERT INTO mem_tree_chunk_reembed_skipped (
                chunk_id, model_signature, reason, skipped_at_ms
             ) VALUES (?1, 'test/model@3', 'terminal', 1700000000000)",
            params![chunk.id],
        )?;
        tx.execute(
            "INSERT INTO mem_tree_ingested_sources (source_kind, source_id, ingested_at_ms)
             VALUES ('chat', 'slack:#eng', 1700000000000)",
            [],
        )?;
        tx.commit()?;
        Ok(())
    })
    .unwrap();
}

const PURGED_TABLES: [&str; 14] = [
    "mem_tree_score",
    "mem_tree_entity_index",
    "mem_tree_entity_edges",
    "mem_tree_entity_hotness",
    "mem_tree_jobs",
    "mem_tree_buffers",
    "mem_tree_summary_embeddings",
    "mem_tree_summary_reembed_skipped",
    "mem_tree_summaries",
    "mem_tree_trees",
    "mem_tree_chunk_embeddings",
    "mem_tree_chunk_reembed_skipped",
    "mem_tree_chunks",
    "mem_tree_ingested_sources",
];

/// A purge that cannot finish must not half-finish. A child table with a
/// restricting foreign key onto `mem_tree_chunks` makes the chunk delete — the
/// thirteenth of fourteen statements — fail, so every table emptied before it
/// has to come back.
#[test]
fn purge_all_rolls_back_when_one_delete_fails() {
    let (_tmp, cfg) = test_config();
    let chunk = chunk_with("slack:#eng", 0, 1_000, "body");
    upsert_chunks(&cfg, std::slice::from_ref(&chunk)).unwrap();
    seed_full_tier(&cfg, &chunk);

    let before: Vec<i64> = PURGED_TABLES
        .iter()
        .map(|table| count_rows(&cfg, table))
        .collect();
    assert!(before.iter().all(|count| *count == 1));

    with_connection(&cfg, |conn| {
        conn.execute_batch(
            "CREATE TABLE purge_block (
                chunk_id TEXT NOT NULL REFERENCES mem_tree_chunks(id)
             );",
        )?;
        conn.execute(
            "INSERT INTO purge_block (chunk_id) VALUES (?1)",
            params![chunk.id],
        )?;
        Ok(())
    })
    .unwrap();

    assert!(
        purge_all(&cfg).is_err(),
        "the blocked chunk delete must surface as an error"
    );
    let after: Vec<i64> = PURGED_TABLES
        .iter()
        .map(|table| count_rows(&cfg, table))
        .collect();
    assert_eq!(
        after, before,
        "a failed purge must leave every table intact"
    );

    with_connection(&cfg, |conn| {
        conn.execute_batch("DROP TABLE purge_block;")?;
        Ok(())
    })
    .unwrap();

    assert_eq!(
        purge_all(&cfg).unwrap(),
        PURGED_TABLES.len(),
        "the count is every row removed, and each table above holds exactly one"
    );
    for table in PURGED_TABLES {
        assert_eq!(count_rows(&cfg, table), 0, "{table} must be empty");
    }
    assert_eq!(count_chunks(&cfg).unwrap(), 0);
    // Idempotent: purging an already-empty store is not an error.
    assert_eq!(purge_all(&cfg).unwrap(), 0);
}

#[test]
fn purge_all_leaves_the_write_audit_trail_alone() {
    let (_tmp, cfg) = test_config();
    let chunk = chunk_with("slack:#eng", 0, 1_000, "body");
    upsert_chunks(&cfg, std::slice::from_ref(&chunk)).unwrap();
    with_connection(&cfg, |conn| {
        conn.execute(
            "INSERT INTO mcp_writes (timestamp_ms, client_info, tool_name, success)
             VALUES (1700000000000, 'test-client', 'remember', 1)",
            [],
        )?;
        Ok(())
    })
    .unwrap();

    let expected: i64 = PURGED_TABLES
        .iter()
        .map(|table| count_rows(&cfg, table))
        .sum();
    assert_eq!(purge_all(&cfg).unwrap() as i64, expected);
    assert_eq!(
        count_rows(&cfg, "mcp_writes"),
        1,
        "an audit record of a write is not the memory it wrote"
    );
}
