#![allow(unused_imports)]
//! Unit tests for the chunk store (`super`): upsert / list / delete.

use super::connection::{
    clear_connection_cache, get_or_init_connection, invalidate_connection,
    schema_apply_count_for_path_for_tests, with_connection, CB_THRESHOLD,
};
use super::embeddings::{active_embedding_dims, embedding_to_blob};
use super::migrations::purge_global_topic_trees;
use super::recovery::{is_transient_cold_start, try_cleanup_stale_files};
use super::store::upsert_staged_chunks_tx;
use super::types::{chunk_id, Chunk, Metadata, SourceKind, SourceRef, StagedChunk};
use super::{
    claim_source_ingest_tx, clear_chunk_reembed_skipped, clear_reembed_skipped_for_signature,
    content_root, count_chunks, count_raw_paths_ingested_with_prefix, db_path_for,
    delete_chunks_by_owner, delete_chunks_by_source, extraction_coverage,
    filter_raw_paths_not_ingested, get_chunk, get_chunk_content_path, get_chunk_content_pointers,
    get_chunk_embedding, get_chunk_embedding_for_signature,
    get_chunk_embeddings_for_signature_batch, get_chunk_raw_refs, get_chunks_batch,
    get_summary_content_pointers, is_source_ingested, list_chunk_raw_ref_paths_with_prefix,
    list_chunks, list_summaries_with_content_path, mark_chunk_reembed_skipped,
    mark_raw_paths_ingested, set_chunk_embedding, set_chunk_embedding_for_signature,
    set_chunk_raw_refs, tree_active_signature, upsert_chunks, ListChunksQuery, RawRef, DB_DIR,
    GLOBAL_TOPIC_PURGE_MIGRATION_VERSION,
};
use crate::memory::config::MemoryConfig;
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
fn upsert_then_get() {
    let (_tmp, cfg) = test_config();
    let c = sample_chunk("slack:#eng", 0, 1_700_000_000_000);
    assert_eq!(upsert_chunks(&cfg, &[c.clone()]).unwrap(), 1);
    let got = get_chunk(&cfg, &c.id).unwrap().expect("chunk stored");
    assert_eq!(got, c);
}

#[test]
fn upsert_persists_path_scope() {
    let (_tmp, cfg) = test_config();
    let mut c = sample_chunk("notion:conn-1:page-abc", 0, 1_700_000_000_000);
    c.metadata.source_kind = SourceKind::Document;
    c.metadata.path_scope = Some("notion:conn-1".to_string());

    assert_eq!(upsert_chunks(&cfg, &[c.clone()]).unwrap(), 1);

    let got = get_chunk(&cfg, &c.id).unwrap().expect("chunk stored");
    assert_eq!(got.metadata.source_id, "notion:conn-1:page-abc");
    assert_eq!(got.metadata.path_scope.as_deref(), Some("notion:conn-1"));
}

#[test]
fn list_chunks_source_scope_filters_before_limit() {
    let (_tmp, cfg) = test_config();
    let tag = || vec!["memory_sources".to_string(), "chat".to_string()];
    let mut bad1 = sample_chunk("slack:#secret", 0, 3_000);
    bad1.metadata.tags = tag();
    let mut bad2 = sample_chunk("slack:#secret", 1, 2_000);
    bad2.metadata.tags = tag();
    let mut good = sample_chunk("slack:#eng", 0, 1_000);
    good.metadata.tags = tag();
    upsert_chunks(&cfg, &[bad1, bad2, good]).unwrap();

    let mut allowed = std::collections::HashSet::new();
    allowed.insert("slack:#eng".to_string());
    let q = ListChunksQuery {
        limit: Some(1),
        source_scope: Some(allowed),
        ..Default::default()
    };
    let rows = list_chunks(&cfg, &q).unwrap();
    assert_eq!(
        rows.len(),
        1,
        "the allowed-source chunk must survive the gate"
    );
    assert_eq!(rows[0].metadata.source_id, "slack:#eng");

    let unscoped = ListChunksQuery {
        limit: Some(1),
        ..Default::default()
    };
    let rows = list_chunks(&cfg, &unscoped).unwrap();
    assert_eq!(rows[0].metadata.source_id, "slack:#secret");
}

#[test]
fn upsert_is_idempotent() {
    let (_tmp, cfg) = test_config();
    let c = sample_chunk("slack:#eng", 0, 1_700_000_000_000);
    upsert_chunks(&cfg, &[c.clone()]).unwrap();
    upsert_chunks(&cfg, &[c.clone()]).unwrap();
    assert_eq!(count_chunks(&cfg).unwrap(), 1);
}

#[test]
fn reingest_preserves_existing_embedding() {
    let (_tmp, cfg) = test_config();
    let mut c = sample_chunk("slack:#eng", 0, 1_700_000_000_000);
    upsert_chunks(&cfg, &[c.clone()]).unwrap();
    set_chunk_embedding(&cfg, &c.id, &[0.1, 0.2, 0.3]).unwrap();

    c.content = "updated content".into();
    c.token_count = 99;
    upsert_chunks(&cfg, &[c.clone()]).unwrap();

    let embedding = get_chunk_embedding(&cfg, &c.id).unwrap().unwrap();
    assert_eq!(embedding, vec![0.1, 0.2, 0.3]);
    let got = get_chunk(&cfg, &c.id).unwrap().unwrap();
    assert_eq!(got.content, "updated content");
    assert_eq!(got.token_count, 99);
}

#[test]
fn chunk_embeddings_are_scoped_by_model_signature() {
    let (_tmp, cfg) = test_config();
    let c = sample_chunk("slack:#eng", 0, 1_700_000_000_000);
    upsert_chunks(&cfg, &[c.clone()]).unwrap();

    set_chunk_embedding_for_signature(
        &cfg,
        &c.id,
        "openai/text-embedding-3-small@1536",
        &[0.1, 0.2],
    )
    .unwrap();
    set_chunk_embedding_for_signature(&cfg, &c.id, "local/bge-small@384", &[0.3, 0.4, 0.5])
        .unwrap();

    assert_eq!(
        get_chunk_embedding_for_signature(&cfg, &c.id, "openai/text-embedding-3-small@1536")
            .unwrap(),
        Some(vec![0.1, 0.2])
    );
    assert_eq!(
        get_chunk_embedding_for_signature(&cfg, &c.id, "local/bge-small@384").unwrap(),
        Some(vec![0.3, 0.4, 0.5])
    );
    assert!(
        get_chunk_embedding_for_signature(&cfg, &c.id, "missing/model@1")
            .unwrap()
            .is_none()
    );

    // The public getter reads the sidecar at the *active* signature; nothing was
    // written there yet, so it is absent — never a cross-space read.
    assert!(get_chunk_embedding(&cfg, &c.id).unwrap().is_none());

    set_chunk_embedding(&cfg, &c.id, &[0.7, 0.8]).unwrap();
    assert_eq!(
        get_chunk_embedding(&cfg, &c.id).unwrap(),
        Some(vec![0.7, 0.8])
    );

    assert_eq!(
        get_chunk_embedding_for_signature(&cfg, &c.id, "local/bge-small@384").unwrap(),
        Some(vec![0.3, 0.4, 0.5])
    );
}

#[test]
fn list_filters_by_source_kind() {
    let (_tmp, cfg) = test_config();
    let c1 = sample_chunk("slack:#eng", 0, 1_700_000_000_000);
    let mut c2 = sample_chunk("gmail:t1", 0, 1_700_000_001_000);
    c2.metadata.source_kind = SourceKind::Email;
    upsert_chunks(&cfg, &[c1.clone(), c2.clone()]).unwrap();
    let q = ListChunksQuery {
        source_kind: Some(SourceKind::Email),
        ..Default::default()
    };
    let rows = list_chunks(&cfg, &q).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].metadata.source_kind, SourceKind::Email);
}

#[test]
fn list_filters_by_time_range() {
    let (_tmp, cfg) = test_config();
    let a = sample_chunk("s", 0, 1_700_000_000_000);
    let b = sample_chunk("s", 1, 1_700_000_010_000);
    let c = sample_chunk("s", 2, 1_700_000_020_000);
    upsert_chunks(&cfg, &[a.clone(), b.clone(), c.clone()]).unwrap();
    let q = ListChunksQuery {
        since_ms: Some(1_700_000_005_000),
        until_ms: Some(1_700_000_015_000),
        ..Default::default()
    };
    let rows = list_chunks(&cfg, &q).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, b.id);
}

#[test]
fn list_orders_by_timestamp_desc() {
    let (_tmp, cfg) = test_config();
    let a = sample_chunk("s", 0, 1_700_000_000_000);
    let b = sample_chunk("s", 1, 1_700_000_010_000);
    upsert_chunks(&cfg, &[a.clone(), b.clone()]).unwrap();
    let rows = list_chunks(&cfg, &ListChunksQuery::default()).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].id, b.id); // newest first
    assert_eq!(rows[1].id, a.id);
}

#[test]
fn list_orders_equal_timestamps_by_sequence() {
    let (_tmp, cfg) = test_config();
    let a = sample_chunk("s", 0, 1_700_000_000_000);
    let b = sample_chunk("s", 1, 1_700_000_000_000);
    upsert_chunks(&cfg, &[b.clone(), a.clone()]).unwrap();
    let rows = list_chunks(&cfg, &ListChunksQuery::default()).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].seq_in_source, 0);
    assert_eq!(rows[1].seq_in_source, 1);
}

#[test]
fn list_limit_is_clamped_to_sane_range() {
    let (_tmp, cfg) = test_config();
    let chunks = (0..3)
        .map(|idx| sample_chunk("s", idx, 1_700_000_000_000 + i64::from(idx)))
        .collect::<Vec<_>>();
    upsert_chunks(&cfg, &chunks).unwrap();

    let zero_limit = list_chunks(
        &cfg,
        &ListChunksQuery {
            limit: Some(0),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(zero_limit.len(), 1);

    let huge_limit = list_chunks(
        &cfg,
        &ListChunksQuery {
            limit: Some(usize::MAX),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(huge_limit.len(), 3);
}

#[test]
fn delete_chunks_by_source_removes_chunks_side_rows_and_ingest_gate() {
    let (_tmp, cfg) = test_config();
    let target_a = sample_chunk("slack:c-1", 0, 1_700_000_000_000);
    let target_b = sample_chunk("slack:c-1", 1, 1_700_000_001_000);
    let other = sample_chunk("slack:c-2", 0, 1_700_000_002_000);
    upsert_chunks(&cfg, &[target_a.clone(), target_b.clone(), other.clone()]).unwrap();

    with_connection(&cfg, |conn| {
        let tx = conn.unchecked_transaction()?;
        for chunk in [&target_a, &target_b, &other] {
            tx.execute(
                "INSERT INTO mem_tree_score (
                    chunk_id, total, token_count_signal, unique_words_signal,
                    metadata_weight, source_weight, interaction_weight,
                    entity_density, dropped, reason, computed_at_ms
                ) VALUES (?1, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 0, NULL, 1700000000000)",
                params![chunk.id],
            )?;
            tx.execute(
                "INSERT INTO mem_tree_entity_index (
                    entity_id, node_id, node_kind, entity_kind, surface, score, timestamp_ms
                ) VALUES (?1, ?2, 'chunk', 'person', 'chat', 0.9, 1700000000000)",
                params![format!("entity:{}", chunk.id), chunk.id],
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
        }
        assert!(claim_source_ingest_tx(
            &tx,
            SourceKind::Chat,
            "slack:c-1",
            1_700_000_000_000
        )?);
        assert!(claim_source_ingest_tx(
            &tx,
            SourceKind::Chat,
            "slack:c-2",
            1_700_000_000_000
        )?);
        tx.commit()?;
        Ok(())
    })
    .unwrap();

    let deleted = delete_chunks_by_source(&cfg, SourceKind::Chat, "slack:c-1").unwrap();

    assert_eq!(deleted, 2);
    assert_eq!(count_chunks(&cfg).unwrap(), 1);
    assert!(get_chunk(&cfg, &target_a.id).unwrap().is_none());
    assert!(get_chunk(&cfg, &target_b.id).unwrap().is_none());
    assert!(get_chunk(&cfg, &other.id).unwrap().is_some());
    assert!(!is_source_ingested(&cfg, SourceKind::Chat, "slack:c-1").unwrap());
    assert!(is_source_ingested(&cfg, SourceKind::Chat, "slack:c-2").unwrap());

    with_connection(&cfg, |conn| {
        let count_by_table = |table: &str| -> rusqlite::Result<i64> {
            conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
        };
        assert_eq!(count_by_table("mem_tree_score")?, 1);
        assert_eq!(count_by_table("mem_tree_entity_index")?, 1);
        assert_eq!(count_by_table("mem_tree_chunk_embeddings")?, 1);
        assert_eq!(count_by_table("mem_tree_chunk_reembed_skipped")?, 1);
        Ok(())
    })
    .unwrap();
}

#[test]
fn delete_chunks_by_owner_preserves_other_owners_for_same_source() {
    let (_tmp, cfg) = test_config();
    let mut target = sample_chunk("slack:shared", 0, 1_700_000_000_000);
    target.metadata.owner = "slack-sync:c-1".to_string();
    let mut same_source_other_owner = sample_chunk("slack:shared", 1, 1_700_000_001_000);
    same_source_other_owner.metadata.owner = "slack-sync:c-2".to_string();
    let mut target_other_source = sample_chunk("slack:c-1-only", 0, 1_700_000_002_000);
    target_other_source.metadata.owner = "slack-sync:c-1".to_string();
    upsert_chunks(
        &cfg,
        &[
            target.clone(),
            same_source_other_owner.clone(),
            target_other_source.clone(),
        ],
    )
    .unwrap();
    with_connection(&cfg, |conn| {
        let tx = conn.unchecked_transaction()?;
        assert!(claim_source_ingest_tx(
            &tx,
            SourceKind::Chat,
            "slack:shared",
            1_700_000_000_000
        )?);
        assert!(claim_source_ingest_tx(
            &tx,
            SourceKind::Chat,
            "slack:c-1-only",
            1_700_000_000_000
        )?);
        tx.commit()?;
        Ok(())
    })
    .unwrap();

    let deleted = delete_chunks_by_owner(&cfg, SourceKind::Chat, "slack-sync:c-1").unwrap();

    assert_eq!(deleted, 2);
    assert!(get_chunk(&cfg, &target.id).unwrap().is_none());
    assert!(get_chunk(&cfg, &target_other_source.id).unwrap().is_none());
    assert!(get_chunk(&cfg, &same_source_other_owner.id)
        .unwrap()
        .is_some());
    assert!(is_source_ingested(&cfg, SourceKind::Chat, "slack:shared").unwrap());
    assert!(!is_source_ingested(&cfg, SourceKind::Chat, "slack:c-1-only").unwrap());
}

#[test]
fn delete_chunks_by_source_removes_safe_content_files_but_rejects_escape_paths() {
    let (_tmp, cfg) = test_config();
    let safe = sample_chunk("slack:c-1", 0, 1_700_000_000_000);
    let unsafe_chunk = sample_chunk("slack:c-1", 1, 1_700_000_001_000);
    upsert_chunks(&cfg, &[safe.clone(), unsafe_chunk.clone()]).unwrap();

    let root = content_root(&cfg);
    let safe_rel = "chunks/safe.md";
    let safe_path = root.join(safe_rel);
    std::fs::create_dir_all(safe_path.parent().unwrap()).unwrap();
    std::fs::write(&safe_path, "safe").unwrap();

    let outside_path = root.parent().unwrap().join("outside.md");
    std::fs::write(&outside_path, "outside").unwrap();

    with_connection(&cfg, |conn| {
        conn.execute(
            "UPDATE mem_tree_chunks SET content_path = ?1 WHERE id = ?2",
            params![safe_rel, safe.id],
        )?;
        conn.execute(
            "UPDATE mem_tree_chunks SET content_path = ?1 WHERE id = ?2",
            params!["../outside.md", unsafe_chunk.id],
        )?;
        Ok(())
    })
    .unwrap();

    let deleted = delete_chunks_by_source(&cfg, SourceKind::Chat, "slack:c-1").unwrap();

    assert_eq!(deleted, 2);
    assert!(!safe_path.exists());
    assert!(outside_path.exists());
}

#[test]
fn raw_refs_round_trip_and_prefix_listing_tolerates_corrupt_rows() {
    let (_tmp, cfg) = test_config();
    let c1 = sample_chunk("gmail:thread-1", 0, 1_700_000_000_000);
    let c2 = sample_chunk("gmail:thread-2", 0, 1_700_000_001_000);
    let corrupt = sample_chunk("gmail:thread-3", 0, 1_700_000_002_000);
    upsert_chunks(&cfg, &[c1.clone(), c2.clone(), corrupt.clone()]).unwrap();

    let refs = vec![
        RawRef {
            path: "raw/gmail/thread_1/message_1.md".into(),
            start: 10,
            end: Some(42),
        },
        RawRef {
            path: "raw/gmail/thread_1/message_2.md".into(),
            start: 0,
            end: None,
        },
    ];
    set_chunk_raw_refs(&cfg, &c1.id, &refs).unwrap();
    set_chunk_raw_refs(
        &cfg,
        &c2.id,
        &[RawRef {
            path: "raw/slack/channel/message.md".into(),
            start: 0,
            end: None,
        }],
    )
    .unwrap();
    with_connection(&cfg, |conn| {
        conn.execute(
            "UPDATE mem_tree_chunks SET raw_refs_json = 'not valid json' WHERE id = ?1",
            params![corrupt.id],
        )?;
        Ok(())
    })
    .unwrap();

    let got = get_chunk_raw_refs(&cfg, &c1.id).unwrap().unwrap();
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].path, "raw/gmail/thread_1/message_1.md");
    assert_eq!(got[0].start, 10);
    assert_eq!(got[0].end, Some(42));
    assert!(get_chunk_raw_refs(&cfg, "missing").unwrap().is_none());

    let paths = list_chunk_raw_ref_paths_with_prefix(&cfg, "raw/gmail/").unwrap();
    assert_eq!(paths.len(), 2);
    assert!(paths.contains("raw/gmail/thread_1/message_1.md"));
    assert!(paths.contains("raw/gmail/thread_1/message_2.md"));
}

#[test]
fn content_pointer_accessors_return_only_complete_non_deleted_rows() {
    let (_tmp, cfg) = test_config();
    let chunk = sample_chunk("notion:page-1", 0, 1_700_000_000_000);
    upsert_chunks(&cfg, &[chunk.clone()]).unwrap();
    with_connection(&cfg, |conn| {
        conn.execute(
            "UPDATE mem_tree_chunks SET content_path = ?1, content_sha256 = ?2 WHERE id = ?3",
            params!["chunks/notion/page-1.md", "abc123", chunk.id],
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO mem_tree_trees (id, kind, scope, created_at_ms)
             VALUES ('tree-content-pointers', 'source', 'notion:page-1', 0)",
            [],
        )?;
        conn.execute(
            "INSERT INTO mem_tree_summaries (
                id, tree_id, tree_kind, level, child_ids_json, content, token_count,
                entities_json, topics_json, time_range_start_ms, time_range_end_ms,
                score, sealed_at_ms, deleted, content_path, content_sha256
             ) VALUES
                ('summary-live', 'tree-content-pointers', 'source', 1, '[]', 'live', 1,
                 '[]', '[]', 0, 0, 1.0, 0, 0, 'summaries/live.md', 'sum123'),
                ('summary-deleted', 'tree-content-pointers', 'source', 1, '[]', 'deleted', 1,
                 '[]', '[]', 0, 0, 1.0, 0, 1, 'summaries/deleted.md', 'sum456'),
                ('summary-incomplete', 'tree-content-pointers', 'source', 1, '[]', 'incomplete', 1,
                 '[]', '[]', 0, 0, 1.0, 0, 0, 'summaries/incomplete.md', NULL)",
            [],
        )?;
        Ok(())
    })
    .unwrap();

    assert_eq!(
        get_chunk_content_path(&cfg, &chunk.id).unwrap().as_deref(),
        Some("chunks/notion/page-1.md")
    );
    assert_eq!(
        get_chunk_content_pointers(&cfg, &chunk.id).unwrap(),
        Some(("chunks/notion/page-1.md".into(), "abc123".into()))
    );
    assert!(get_chunk_content_pointers(&cfg, "missing")
        .unwrap()
        .is_none());
    assert_eq!(
        get_summary_content_pointers(&cfg, "summary-live").unwrap(),
        Some(("summaries/live.md".into(), "sum123".into()))
    );

    let summaries = list_summaries_with_content_path(&cfg).unwrap();
    assert_eq!(
        summaries,
        vec![(
            "summary-live".into(),
            "summaries/live.md".into(),
            "sum123".into()
        )]
    );
}

#[test]
fn raw_file_ingest_gate_is_order_preserving_and_prefix_scoped() {
    let (_tmp, cfg) = test_config();
    let paths = vec![
        "raw/gmail/a.md".to_string(),
        "raw/gmail/b.md".to_string(),
        "raw/slack/c.md".to_string(),
    ];

    assert_eq!(filter_raw_paths_not_ingested(&cfg, &paths).unwrap(), paths);
    assert_eq!(mark_raw_paths_ingested(&cfg, &paths[..2]).unwrap(), 2);
    assert_eq!(mark_raw_paths_ingested(&cfg, &paths[..2]).unwrap(), 0);
    assert_eq!(
        filter_raw_paths_not_ingested(&cfg, &paths).unwrap(),
        vec!["raw/slack/c.md".to_string()]
    );
    assert_eq!(
        count_raw_paths_ingested_with_prefix(&cfg, "raw/gmail/").unwrap(),
        2
    );
    assert_eq!(
        count_raw_paths_ingested_with_prefix(&cfg, "raw/slack/").unwrap(),
        0
    );
}

#[test]
fn staged_chunk_upsert_persists_preview_and_content_pointers() {
    let (_tmp, cfg) = test_config();
    let mut chunk = sample_chunk("notion:staged", 0, 1_700_000_000_000);
    chunk.metadata.source_kind = SourceKind::Document;
    chunk.metadata.path_scope = Some("notion:workspace".into());
    chunk.content = "x".repeat(620);
    chunk.token_count = 155;
    let staged = StagedChunk {
        chunk: chunk.clone(),
        content_path: "chunks/notion/workspace/page.md".into(),
        content_sha256: "sha-staged".into(),
    };

    let inserted = with_connection(&cfg, |conn| {
        let tx = conn.unchecked_transaction()?;
        let inserted = upsert_staged_chunks_tx(&tx, std::slice::from_ref(&staged))?;
        tx.commit()?;
        Ok(inserted)
    })
    .unwrap();
    assert_eq!(inserted, 1);

    let got = get_chunk(&cfg, &chunk.id).unwrap().unwrap();
    assert_eq!(got.content.len(), 500);
    assert!(got.content.chars().all(|ch| ch == 'x'));
    assert_eq!(got.metadata.path_scope.as_deref(), Some("notion:workspace"));
    assert_eq!(got.token_count, 155);
    assert_eq!(
        get_chunk_content_pointers(&cfg, &chunk.id).unwrap(),
        Some((
            "chunks/notion/workspace/page.md".into(),
            "sha-staged".into()
        ))
    );
}

#[test]
fn missing_chunk_returns_none() {
    let (_tmp, cfg) = test_config();
    assert!(get_chunk(&cfg, "nonexistent").unwrap().is_none());
}

#[test]
fn empty_batch_is_noop() {
    let (_tmp, cfg) = test_config();
    assert_eq!(upsert_chunks(&cfg, &[]).unwrap(), 0);
    assert_eq!(count_chunks(&cfg).unwrap(), 0);
}

#[test]
fn schema_has_content_path_and_content_sha256_columns() {
    let (_tmp, cfg) = test_config();
    with_connection(&cfg, |conn| {
        let mut stmt = conn.prepare("PRAGMA table_info(mem_tree_chunks)")?;
        let names: Vec<String> = stmt
            .query_map(params![], |row| row.get::<_, String>(1))?
            .filter_map(|r| r.ok())
            .collect();
        assert!(
            names.iter().any(|n| n == "path_scope"),
            "missing path_scope; found: {names:?}"
        );
        assert!(
            names.iter().any(|n| n == "content_path"),
            "missing content_path; found: {names:?}"
        );
        assert!(
            names.iter().any(|n| n == "content_sha256"),
            "missing content_sha256; found: {names:?}"
        );
        Ok(())
    })
    .unwrap();
}
