#![allow(unused_imports)]
//! Unit tests for the chunk store (`super`) — upsert / list / lifecycle /
//! embedding / delete / migration accessors against a tempdir-backed SQLite
//! store.
//!
//! Because the connection cache is a process-level singleton, tests that
//! exercise cache behaviour call `clear_connection_cache()` at the start, or
//! use unique tempdirs that cannot collide with other tests.

use super::connection::{
    clear_connection_cache, get_or_init_connection, invalidate_connection,
    schema_apply_count_for_path_for_tests, with_connection, CB_THRESHOLD,
};
use super::embeddings::{active_embedding_dims, embedding_to_blob};
use super::migrations::purge_global_topic_trees;
use super::recovery::{is_transient_cold_start, try_cleanup_stale_files};
use super::types::{chunk_id, Chunk, Metadata, SourceKind, SourceRef};
use super::{
    claim_source_ingest_tx, clear_chunk_reembed_skipped, clear_reembed_skipped_for_signature,
    clear_summary_reembed_skipped, content_root, count_chunks, db_path_for, delete_chunks_by_owner,
    delete_chunks_by_source, extraction_coverage, get_chunk, get_chunk_embedding,
    get_chunk_embedding_for_signature, get_chunk_embeddings_for_signature_batch, get_chunks_batch,
    is_source_ingested, list_chunks, mark_chunk_reembed_skipped, mark_summary_reembed_skipped,
    set_chunk_embedding, set_chunk_embedding_for_signature, tree_active_signature, upsert_chunks,
    ListChunksQuery, DB_DIR, GLOBAL_TOPIC_PURGE_MIGRATION_VERSION,
};
use crate::memory::config::MemoryConfig;
use crate::memory::tree::store::{
    insert_summary_tx, insert_tree, SummaryNode, Tree, TreeKind, TreeStatus,
};
use chrono::{TimeZone, Utc};
use rusqlite::params;
use std::sync::Arc;
use tempfile::TempDir;

fn test_config() -> (TempDir, MemoryConfig) {
    let tmp = TempDir::new().expect("tempdir");
    let cfg = MemoryConfig::new(tmp.path());
    (tmp, cfg)
}

fn sample_chunk(source_id: &str, seq: u32, ts_ms: i64) -> Chunk {
    let ts = Utc.timestamp_millis_opt(ts_ms).unwrap();
    Chunk {
        id: chunk_id(SourceKind::Chat, source_id, seq, "test-content"),
        content: format!("content {source_id} {seq}"),
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

#[test]
fn clear_chunk_reembed_skipped_is_idempotent() {
    let (_tmp, cfg) = test_config();
    let c = sample_chunk("slack:#eng", 0, 1_700_000_000_000);
    upsert_chunks(&cfg, std::slice::from_ref(&c)).unwrap();
    let sig = tree_active_signature(&cfg);
    mark_chunk_reembed_skipped(&cfg, &c.id, &sig, "test orphan").unwrap();
    clear_chunk_reembed_skipped(&cfg, &c.id, &sig).unwrap();
    clear_chunk_reembed_skipped(&cfg, &c.id, &sig).unwrap();
    let count: i64 = with_connection(&cfg, |conn| {
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM mem_tree_chunk_reembed_skipped
              WHERE chunk_id = ?1 AND model_signature = ?2",
            params![c.id, sig],
            |r| r.get(0),
        )?)
    })
    .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn summary_reembed_tombstone_roundtrips_and_clears() {
    let (_tmp, cfg) = test_config();
    let now = Utc::now();
    insert_tree(
        &cfg,
        &Tree {
            id: "tree-1".into(),
            kind: TreeKind::Source,
            scope: "source-1".into(),
            root_id: None,
            max_level: 0,
            status: TreeStatus::Active,
            created_at: now,
            last_sealed_at: None,
        },
    )
    .unwrap();
    with_connection(&cfg, |conn| {
        let tx = conn.unchecked_transaction()?;
        insert_summary_tx(
            &tx,
            &SummaryNode {
                id: "summary-1".into(),
                tree_id: "tree-1".into(),
                tree_kind: TreeKind::Source,
                level: 1,
                parent_id: None,
                child_ids: vec![],
                content: "summary".into(),
                token_count: 2,
                entities: vec![],
                topics: vec![],
                time_range_start: now,
                time_range_end: now,
                score: 1.0,
                sealed_at: now,
                deleted: false,
                embedding: None,
                doc_id: None,
                version_ms: None,
            },
            "model@3",
        )?;
        tx.commit()?;
        Ok(())
    })
    .unwrap();
    mark_summary_reembed_skipped(&cfg, "summary-1", "model@3", "oversize").unwrap();
    with_connection(&cfg, |conn| {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM mem_tree_summary_reembed_skipped
             WHERE summary_id='summary-1' AND model_signature='model@3'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(count, 1);
        Ok(())
    })
    .unwrap();
    clear_summary_reembed_skipped(&cfg, "summary-1", "model@3").unwrap();
    clear_summary_reembed_skipped(&cfg, "summary-1", "model@3").unwrap();
}

#[test]
fn clear_reembed_skipped_for_signature_removes_all_tombstones_for_sig() {
    let (_tmp, cfg) = test_config();
    let c1 = sample_chunk("slack:#a", 0, 1_700_000_000_000);
    let c2 = sample_chunk("slack:#b", 1, 1_700_000_000_001);
    upsert_chunks(&cfg, &[c1.clone(), c2.clone()]).unwrap();
    let sig = tree_active_signature(&cfg);
    let other_sig = "provider=other;model=x;dims=8";
    mark_chunk_reembed_skipped(&cfg, &c1.id, &sig, "r1").unwrap();
    mark_chunk_reembed_skipped(&cfg, &c2.id, &sig, "r2").unwrap();
    mark_chunk_reembed_skipped(&cfg, &c1.id, other_sig, "other").unwrap();
    // Seed a summary + a summary-side tombstone directly via SQL (the tree
    // store that owns these rows is not part of this slice).
    let summary_id = "summary-bulk-clear-test";
    with_connection(&cfg, |conn| {
        conn.execute(
            "INSERT OR IGNORE INTO mem_tree_trees (id, kind, scope, created_at_ms)
             VALUES ('tree-bulk-clear', 'source', 'bulk-clear', 0)",
            [],
        )?;
        conn.execute(
            "INSERT INTO mem_tree_summaries (
                id, tree_id, tree_kind, level, child_ids_json, content, token_count,
                entities_json, topics_json, time_range_start_ms, time_range_end_ms,
                score, sealed_at_ms, deleted
             ) VALUES (?1, 'tree-bulk-clear', 'source', 0, '[]', 'x', 1, '[]', '[]', 0, 0, 0.0, 0, 0)",
            params![summary_id],
        )?;
        conn.execute(
            "INSERT INTO mem_tree_summary_reembed_skipped (summary_id, model_signature, reason, skipped_at_ms)
             VALUES (?1, ?2, 'summary tombstone', 0)",
            params![summary_id, sig],
        )?;
        Ok(())
    })
    .unwrap();

    let deleted = clear_reembed_skipped_for_signature(&cfg, &sig).unwrap();
    assert_eq!(deleted, 3);

    let remaining_chunks: i64 = with_connection(&cfg, |conn| {
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM mem_tree_chunk_reembed_skipped WHERE model_signature = ?1",
            params![sig],
            |r| r.get(0),
        )?)
    })
    .unwrap();
    assert_eq!(remaining_chunks, 0);

    let remaining_summaries: i64 = with_connection(&cfg, |conn| {
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM mem_tree_summary_reembed_skipped WHERE model_signature = ?1",
            params![sig],
            |r| r.get(0),
        )?)
    })
    .unwrap();
    assert_eq!(remaining_summaries, 0);

    let other_kept: i64 = with_connection(&cfg, |conn| {
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM mem_tree_chunk_reembed_skipped
              WHERE chunk_id = ?1 AND model_signature = ?2",
            params![c1.id, other_sig],
            |r| r.get(0),
        )?)
    })
    .unwrap();
    assert_eq!(other_kept, 1);
}

#[test]
fn validate_reembed_skip_key_rejects_empty_and_oversized() {
    use super::embeddings::{validate_reembed_skip_key, REEMBED_SKIP_KEY_MAX_LEN};
    assert!(validate_reembed_skip_key("chunk_id", "  ").is_err());
    let huge = "a".repeat(REEMBED_SKIP_KEY_MAX_LEN + 1);
    assert!(validate_reembed_skip_key("chunk_id", &huge).is_err());
    assert!(validate_reembed_skip_key("chunk_id", "ok\0bad").is_err());
    assert_eq!(
        validate_reembed_skip_key("chunk_id", "  trimmed  ").unwrap(),
        "trimmed"
    );
}

#[test]
fn get_chunks_batch_returns_present_ids_in_map() {
    let (_tmp, cfg) = test_config();
    let c1 = sample_chunk("slack:#eng", 0, 1_700_000_000_000);
    let c2 = sample_chunk("slack:#eng", 1, 1_700_000_000_000);
    let c3 = sample_chunk("slack:#ops", 0, 1_700_000_000_000);
    upsert_chunks(&cfg, &[c1.clone(), c2.clone(), c3.clone()]).unwrap();

    let ids = vec![c1.id.clone(), c2.id.clone(), c3.id.clone()];
    let map = get_chunks_batch(&cfg, &ids).unwrap();
    assert_eq!(map.len(), 3);
    assert_eq!(map.get(&c1.id), Some(&c1));
    assert_eq!(map.get(&c2.id), Some(&c2));
    assert_eq!(map.get(&c3.id), Some(&c3));
}

#[test]
fn get_chunks_batch_empty_input_and_missing_ids() {
    let (_tmp, cfg) = test_config();
    let empty = get_chunks_batch(&cfg, &[]).unwrap();
    assert!(empty.is_empty());

    let c = sample_chunk("slack:#eng", 0, 1_700_000_000_000);
    upsert_chunks(&cfg, std::slice::from_ref(&c)).unwrap();
    let ids = vec![
        c.id.clone(),
        "ghost:no-such-1".into(),
        "ghost:no-such-2".into(),
    ];
    let map = get_chunks_batch(&cfg, &ids).unwrap();
    assert_eq!(map.len(), 1);
    assert_eq!(map.get(&c.id), Some(&c));
    assert!(!map.contains_key("ghost:no-such-1"));
    assert!(!map.contains_key("ghost:no-such-2"));
}

#[test]
fn batch_embedding_lookup_returns_only_signature_scoped_rows() {
    let (_tmp, cfg) = test_config();
    let c1 = sample_chunk("slack:#eng", 0, 1_700_000_000_000);
    let c2 = sample_chunk("slack:#eng", 1, 1_700_000_000_000);
    let c3 = sample_chunk("slack:#eng", 2, 1_700_000_000_000);
    upsert_chunks(&cfg, &[c1.clone(), c2.clone(), c3.clone()]).unwrap();

    let sig_a = "openai/text-embedding-3-small@1536";
    let sig_b = "local/bge-small@384";
    set_chunk_embedding_for_signature(&cfg, &c1.id, sig_a, &[0.1, 0.2]).unwrap();
    set_chunk_embedding_for_signature(&cfg, &c2.id, sig_a, &[0.3, 0.4]).unwrap();
    set_chunk_embedding_for_signature(&cfg, &c3.id, sig_b, &[0.5, 0.6, 0.7]).unwrap();

    let ids = vec![c1.id.clone(), c2.id.clone(), c3.id.clone()];
    let map_a = get_chunk_embeddings_for_signature_batch(&cfg, &ids, sig_a).unwrap();
    assert_eq!(map_a.len(), 2, "only c1 and c2 are under sig_a");
    assert_eq!(map_a.get(&c1.id).cloned(), Some(vec![0.1, 0.2]));
    assert_eq!(map_a.get(&c2.id).cloned(), Some(vec![0.3, 0.4]));
    assert!(!map_a.contains_key(&c3.id), "c3 has only sig_b");

    let map_b = get_chunk_embeddings_for_signature_batch(&cfg, &ids, sig_b).unwrap();
    assert_eq!(map_b.len(), 1);
    assert_eq!(map_b.get(&c3.id).cloned(), Some(vec![0.5, 0.6, 0.7]));
}

#[test]
fn batch_embedding_lookup_empty_input_returns_empty_map() {
    let (_tmp, cfg) = test_config();
    let map = get_chunk_embeddings_for_signature_batch(&cfg, &[], "any/sig@1").unwrap();
    assert!(map.is_empty());
}

#[test]
fn batch_embedding_lookup_unknown_ids_absent_from_map() {
    let (_tmp, cfg) = test_config();
    let c = sample_chunk("slack:#eng", 0, 1_700_000_000_000);
    upsert_chunks(&cfg, std::slice::from_ref(&c)).unwrap();
    let sig = "openai/text-embedding-3-small@1536";
    set_chunk_embedding_for_signature(&cfg, &c.id, sig, &[0.1]).unwrap();

    let ids = vec![
        c.id.clone(),
        "ghost:no-such-chunk-1".into(),
        "ghost:no-such-chunk-2".into(),
    ];
    let map = get_chunk_embeddings_for_signature_batch(&cfg, &ids, sig).unwrap();
    assert_eq!(map.len(), 1);
    assert_eq!(map.get(&c.id).cloned(), Some(vec![0.1]));
}

#[test]
fn batch_embedding_lookup_splits_id_list_above_per_batch_threshold() {
    let (_tmp, cfg) = test_config();
    let c1 = sample_chunk("slack:#a", 0, 1_700_000_000_000);
    let c2 = sample_chunk("slack:#b", 0, 1_700_000_000_000);
    let c3 = sample_chunk("slack:#c", 0, 1_700_000_000_000);
    upsert_chunks(&cfg, &[c1.clone(), c2.clone(), c3.clone()]).unwrap();
    let sig = "openai/text-embedding-3-small@1536";
    set_chunk_embedding_for_signature(&cfg, &c1.id, sig, &[1.0]).unwrap();
    set_chunk_embedding_for_signature(&cfg, &c2.id, sig, &[2.0]).unwrap();
    set_chunk_embedding_for_signature(&cfg, &c3.id, sig, &[3.0]).unwrap();

    let mut ids: Vec<String> = (0..498).map(|i| format!("ghost:{i}")).collect();
    ids.push(c1.id.clone());
    ids.push(c2.id.clone());
    ids.push(c3.id.clone());
    assert_eq!(ids.len(), 501);

    let map = get_chunk_embeddings_for_signature_batch(&cfg, &ids, sig).unwrap();
    assert_eq!(map.len(), 3, "only the 3 real ids should be present");
    assert_eq!(map.get(&c1.id).cloned(), Some(vec![1.0]));
    assert_eq!(map.get(&c2.id).cloned(), Some(vec![2.0]));
    assert_eq!(map.get(&c3.id).cloned(), Some(vec![3.0]));
}

#[test]
fn global_topic_purge_removes_only_global_and_topic() {
    let (_tmp, cfg) = test_config();
    upsert_chunks(&cfg, &[sample_chunk("slack:#eng", 0, 1_700_000_000_000)]).unwrap();

    let summaries = content_root(&cfg).join("wiki").join("summaries");
    for d in [
        "global-2026-05-28",
        "global",
        "topic-alice",
        "source-slack-eng",
    ] {
        std::fs::create_dir_all(summaries.join(d).join("L0")).unwrap();
    }

    with_connection(&cfg, |conn| {
        for (id, kind) in [("source:s1", "source"), ("global:g1", "global"), ("topic:t1", "topic")] {
            conn.execute(
                "INSERT INTO mem_tree_trees (id, kind, scope, max_level, status, created_at_ms) \
                 VALUES (?1, ?2, ?2, 0, 'active', 0)",
                params![id, kind],
            )?;
            conn.execute(
                "INSERT INTO mem_tree_summaries \
                 (id, tree_id, tree_kind, level, content, token_count, \
                  time_range_start_ms, time_range_end_ms, sealed_at_ms) \
                 VALUES (?1, ?2, ?3, 0, 'x', 1, 0, 0, 0)",
                params![format!("sum-{id}"), id, kind],
            )?;
        }
        for (jid, kind) in [("j1", "topic_route"), ("j2", "digest_daily"), ("j3", "extract_chunk")] {
            conn.execute(
                "INSERT INTO mem_tree_jobs (id, kind, payload_json, available_at_ms, created_at_ms) \
                 VALUES (?1, ?2, '{}', 0, 0)",
                params![jid, kind],
            )?;
        }
        conn.pragma_update(None, "user_version", 1i64)?;
        purge_global_topic_trees(conn, &cfg)?;

        let trees: i64 = conn.query_row("SELECT COUNT(*) FROM mem_tree_trees", [], |r| r.get(0))?;
        assert_eq!(trees, 1, "only the source tree should remain");
        let kind: String = conn.query_row("SELECT kind FROM mem_tree_trees", [], |r| r.get(0))?;
        assert_eq!(kind, "source");

        let summaries_left: i64 =
            conn.query_row("SELECT COUNT(*) FROM mem_tree_summaries", [], |r| r.get(0))?;
        assert_eq!(summaries_left, 1);

        let jobs_left: Vec<String> = {
            let mut stmt = conn.prepare("SELECT kind FROM mem_tree_jobs ORDER BY kind")?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        assert_eq!(jobs_left, vec!["extract_chunk".to_string()]);

        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        assert_eq!(version, 2);
        Ok(())
    })
    .unwrap();

    assert!(!summaries.join("global-2026-05-28").exists());
    assert!(!summaries.join("global").exists());
    assert!(!summaries.join("topic-alice").exists());
    assert!(
        summaries.join("source-slack-eng").exists(),
        "source summary folder must survive"
    );
}

#[test]
fn extraction_coverage_empty_store_is_zero() {
    let (_tmp, cfg) = test_config();
    assert_eq!(extraction_coverage(&cfg).unwrap(), 0.0);
}

#[test]
fn extraction_coverage_reflects_indexed_fraction() {
    let (_tmp, cfg) = test_config();
    let c1 = sample_chunk("slack:#eng", 0, 1_700_000_000_000);
    let c2 = sample_chunk("slack:#eng", 1, 1_700_000_001_000);
    upsert_chunks(&cfg, &[c1.clone(), c2.clone()]).unwrap();

    with_connection(&cfg, |conn| {
        conn.execute(
            "INSERT INTO mem_tree_entity_index
                (entity_id, node_id, node_kind, entity_kind, surface, score, timestamp_ms)
             VALUES (?1, ?2, 'leaf', 'person', 'Alice', 0.9, 1)",
            params!["person:Alice", c1.id],
        )?;
        Ok(())
    })
    .unwrap();

    let cov = extraction_coverage(&cfg).unwrap();
    assert!((cov - 0.5).abs() < 1e-6, "expected 0.5, got {cov}");

    with_connection(&cfg, |conn| {
        conn.execute(
            "INSERT INTO mem_tree_entity_index
                (entity_id, node_id, node_kind, entity_kind, surface, score, timestamp_ms)
             VALUES (?1, ?2, 'leaf', 'person', 'Bob', 0.9, 2)",
            params!["person:Bob", c2.id],
        )?;
        Ok(())
    })
    .unwrap();
    assert!((extraction_coverage(&cfg).unwrap() - 1.0).abs() < 1e-6);
}
