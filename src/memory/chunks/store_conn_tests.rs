#![allow(unused_imports)]
//! Unit tests for the chunk store (`super`) — upsert / list / lifecycle /
//! embedding / delete / migration accessors against a tempdir-backed SQLite
//! store.
//!
//! Tests use unique tempdirs so parallel cache checks cannot collide. Tests
//! that must force a reopen reset only their own workspace path.

use super::connection::{
    clear_connection_cache_for, get_or_init_connection, invalidate_connection,
    schema_apply_count_for_path_for_tests, with_connection, CB_THRESHOLD,
};
use super::embeddings::{active_embedding_dims, embedding_to_blob};
use super::migrations::purge_global_topic_trees;
use super::recovery::{is_transient_cold_start, try_cleanup_stale_files};
use super::types::{chunk_id, Chunk, Metadata, SourceKind, SourceRef};
use super::{
    claim_source_ingest_tx, clear_chunk_reembed_skipped, clear_reembed_skipped_for_signature,
    content_root, count_chunks, db_path_for, delete_chunks_by_owner, delete_chunks_by_source,
    extraction_coverage, get_chunk, get_chunk_embedding, get_chunk_embedding_for_signature,
    get_chunk_embeddings_for_signature_batch, get_chunks_batch, is_source_ingested, list_chunks,
    mark_chunk_reembed_skipped, set_chunk_embedding, set_chunk_embedding_for_signature,
    tree_active_signature, upsert_chunks, ListChunksQuery, DB_DIR,
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
fn with_connection_serialises_concurrent_schema_init() {
    use std::sync::atomic::Ordering;

    let (_tmp, cfg) = test_config();
    let db_path = db_path_for(&cfg);
    let errors = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

    let threads: Vec<_> = (0..8)
        .map(|_| {
            let cfg = cfg.clone();
            let errors = errors.clone();
            std::thread::spawn(move || {
                if with_connection(&cfg, |_| Ok(())).is_err() {
                    errors.fetch_add(1, Ordering::Relaxed);
                }
            })
        })
        .collect();
    for t in threads {
        t.join().expect("worker thread panicked");
    }

    assert_eq!(
        errors.load(Ordering::Relaxed),
        0,
        "concurrent with_connection callers must all succeed"
    );
    let applied = schema_apply_count_for_path_for_tests(&db_path);
    assert!(
        applied >= 1,
        "apply_schema must run before concurrent callers can use the DB"
    );
}

#[test]
fn is_transient_cold_start_classifies_known_extended_codes() {
    use rusqlite::ffi;
    use rusqlite::ErrorCode;

    for extended in [14, 1546, 4618, 4874, 5386, 8714] {
        let err = anyhow::Error::from(rusqlite::Error::SqliteFailure(
            ffi::Error {
                code: ErrorCode::SystemIoFailure,
                extended_code: extended,
            },
            None,
        ));
        assert!(
            is_transient_cold_start(&err),
            "extended_code {extended} must classify as transient cold-start"
        );
    }

    let busy = anyhow::Error::from(rusqlite::Error::SqliteFailure(
        ffi::Error {
            code: ErrorCode::DatabaseBusy,
            extended_code: 5,
        },
        None,
    ));
    assert!(
        !is_transient_cold_start(&busy),
        "DatabaseBusy must not classify as cold-start"
    );

    let other: anyhow::Error = anyhow::anyhow!("not a sqlite error");
    assert!(
        !is_transient_cold_start(&other),
        "non-SQLite errors must not classify"
    );
}

#[test]
fn with_connection_keeps_foreign_keys_on_for_every_call() {
    let (_tmp, cfg) = test_config();
    let fk_on_first: i64 = with_connection(&cfg, |conn| {
        Ok(conn.query_row("PRAGMA foreign_keys;", params![], |r| r.get::<_, i64>(0))?)
    })
    .unwrap();
    assert_eq!(
        fk_on_first, 1,
        "foreign_keys must be ON on first connection"
    );
    let fk_on_second: i64 = with_connection(&cfg, |conn| {
        Ok(conn.query_row("PRAGMA foreign_keys;", params![], |r| r.get::<_, i64>(0))?)
    })
    .unwrap();
    assert_eq!(
        fk_on_second, 1,
        "foreign_keys must be ON on fast-path connection"
    );
}

#[test]
fn legacy_embeddings_migrate_to_sidecar_once() {
    let (_tmp, cfg) = test_config();
    let c_match = sample_chunk("slack:#eng", 0, 1_700_000_000_000);
    let c_mismatch = sample_chunk("slack:#eng", 1, 1_700_000_000_001);
    // First open runs the (no-op) migrations and sets user_version to latest.
    upsert_chunks(&cfg, &[c_match.clone(), c_mismatch.clone()]).unwrap();

    let sig = tree_active_signature(&cfg);
    let dims = active_embedding_dims(&cfg);
    let match_vec = vec![0.25f32; dims];
    let mismatch_vec = vec![0.5f32; dims + 1];

    // Simulate a pre-migration DB: legacy columns populated, gate reset to 0.
    with_connection(&cfg, |conn| {
        conn.execute(
            "UPDATE mem_tree_chunks SET embedding = ?1 WHERE id = ?2",
            params![embedding_to_blob(&match_vec), c_match.id],
        )?;
        conn.execute(
            "UPDATE mem_tree_chunks SET embedding = ?1 WHERE id = ?2",
            params![embedding_to_blob(&mismatch_vec), c_mismatch.id],
        )?;
        conn.pragma_update(None, "user_version", 0i64)?;
        Ok(())
    })
    .unwrap();

    invalidate_connection(&cfg);

    assert_eq!(
        get_chunk_embedding_for_signature(&cfg, &c_match.id, &sig).unwrap(),
        Some(match_vec.clone()),
        "matching-dim legacy row must be copied to the sidecar at the active sig"
    );
    assert!(
        get_chunk_embedding_for_signature(&cfg, &c_mismatch.id, &sig)
            .unwrap()
            .is_none(),
        "dim-mismatched legacy row must be skipped"
    );

    with_connection(&cfg, |conn| {
        let legacy: Option<Vec<u8>> = conn
            .query_row(
                "SELECT embedding FROM mem_tree_chunks WHERE id = ?1",
                params![c_match.id],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            legacy.is_some(),
            "legacy column must be KEPT post-migration"
        );
        let v: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            v, GLOBAL_TOPIC_PURGE_MIGRATION_VERSION,
            "version gate must be set to the latest migration"
        );
        Ok(())
    })
    .unwrap();

    // Idempotent: subsequent opens are no-ops.
    with_connection(&cfg, |_| Ok(())).unwrap();
    assert_eq!(
        get_chunk_embedding_for_signature(&cfg, &c_match.id, &sig).unwrap(),
        Some(match_vec)
    );
}

#[test]
fn connection_cache_returns_same_arc_for_same_workspace() {
    let (_tmp, cfg) = test_config();

    let arc1 = get_or_init_connection(&cfg).expect("first get_or_init");
    let arc2 = get_or_init_connection(&cfg).expect("second get_or_init");
    assert!(
        Arc::ptr_eq(&arc1, &arc2),
        "expected the same Arc from the connection cache"
    );
}

#[test]
fn connection_cache_uses_separate_connections_for_different_workspaces() {
    let (_tmp1, cfg1) = test_config();
    let (_tmp2, cfg2) = test_config();

    let arc1 = get_or_init_connection(&cfg1).expect("workspace 1");
    let arc2 = get_or_init_connection(&cfg2).expect("workspace 2");
    assert!(
        !Arc::ptr_eq(&arc1, &arc2),
        "different workspaces must have independent connections"
    );

    let c = sample_chunk("s", 0, 1_700_000_000_000);
    upsert_chunks(&cfg1, std::slice::from_ref(&c)).unwrap();
    assert_eq!(count_chunks(&cfg1).unwrap(), 1);
    assert_eq!(count_chunks(&cfg2).unwrap(), 0);
}

#[test]
fn circuit_breaker_trips_after_threshold() {
    let tmp = TempDir::new().expect("tempdir");

    // Create a regular file where the memory_tree *directory* would be.
    let blocker = tmp.path().join(DB_DIR);
    std::fs::write(&blocker, b"not a dir").expect("write blocker file");

    let cfg = MemoryConfig::new(tmp.path());

    for i in 0..CB_THRESHOLD {
        let result = get_or_init_connection(&cfg);
        assert!(
            result.is_err(),
            "call {i}: expected error before breaker trips"
        );
    }

    let cb_err =
        get_or_init_connection(&cfg).expect_err("expected circuit breaker error after threshold");
    let msg = format!("{cb_err:#}").to_ascii_lowercase();
    assert!(
        msg.contains("circuit breaker"),
        "expected circuit breaker message, got: {msg}"
    );
}

#[test]
fn stale_shm_cleanup_removes_files() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("chunks.db");

    std::fs::write(&db_path, b"").expect("create db file");
    let shm = tmp.path().join("chunks.db-shm");
    let wal = tmp.path().join("chunks.db-wal");
    std::fs::write(&shm, b"stale shm").expect("create shm");
    std::fs::write(&wal, b"stale wal").expect("create wal");

    let cleaned = try_cleanup_stale_files(&db_path);
    assert!(
        cleaned,
        "cleanup should return true when files were removed"
    );
    assert!(!shm.exists(), "shm must be removed");
    assert!(!wal.exists(), "wal must be removed");
}

#[test]
fn memory_tree_uses_truncate_journal_not_wal() {
    let (_tmp, cfg) = test_config();

    with_connection(&cfg, |conn| {
        let mode: String = conn.query_row("PRAGMA journal_mode", [], |r| r.get(0))?;
        assert!(
            mode.eq_ignore_ascii_case("truncate"),
            "journal_mode must be TRUNCATE, got '{mode}'"
        );
        let sync: i64 = conn.query_row("PRAGMA synchronous", [], |r| r.get(0))?;
        assert_eq!(sync, 2, "rollback journal requires synchronous=FULL (2)");
        Ok(())
    })
    .expect("with_connection");

    let shm = cfg.workspace.join("memory_tree").join("chunks.db-shm");
    assert!(
        !shm.exists(),
        "no -shm file must exist under TRUNCATE journal"
    );
}

#[test]
fn existing_wal_db_migrates_to_truncate() {
    let (_tmp, cfg) = test_config();
    let db_path = cfg.workspace.join("memory_tree").join("chunks.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).expect("mkdir");

    {
        let conn = rusqlite::Connection::open(&db_path).expect("open wal db");
        let mode: String = conn
            .query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))
            .expect("set wal");
        assert!(mode.eq_ignore_ascii_case("wal"), "precondition: db in WAL");
        conn.execute_batch("CREATE TABLE legacy_marker(x); INSERT INTO legacy_marker VALUES (1);")
            .expect("seed");
    }

    clear_connection_cache_for(&cfg);
    with_connection(&cfg, |conn| {
        let mode: String = conn.query_row("PRAGMA journal_mode", [], |r| r.get(0))?;
        assert!(
            mode.eq_ignore_ascii_case("truncate"),
            "WAL db must migrate to TRUNCATE, got '{mode}'"
        );
        let marker: i64 = conn.query_row("SELECT x FROM legacy_marker", [], |r| r.get(0))?;
        assert_eq!(marker, 1, "row committed under WAL must survive migration");
        Ok(())
    })
    .expect("with_connection migrates");

    assert!(
        !db_path.with_file_name("chunks.db-shm").exists(),
        "-shm must be gone"
    );
    assert!(
        !db_path.with_file_name("chunks.db-wal").exists(),
        "-wal must be gone"
    );
}

#[cfg(unix)]
#[test]
fn delete_chunks_by_source_removes_symlink_entry_not_target_file() {
    let (_tmp, cfg) = test_config();
    let linked_chunk = sample_chunk("slack:c-1", 0, 1_700_000_000_000);
    upsert_chunks(&cfg, std::slice::from_ref(&linked_chunk)).unwrap();

    let root = content_root(&cfg);
    let target_path = root.join("chunks/target.md");
    let link_rel = "chunks/link.md";
    let link_path = root.join(link_rel);
    std::fs::create_dir_all(target_path.parent().unwrap()).unwrap();
    std::fs::write(&target_path, "target").unwrap();
    std::os::unix::fs::symlink("target.md", &link_path).unwrap();

    with_connection(&cfg, |conn| {
        conn.execute(
            "UPDATE mem_tree_chunks SET content_path = ?1 WHERE id = ?2",
            params![link_rel, linked_chunk.id],
        )?;
        Ok(())
    })
    .unwrap();

    let deleted = delete_chunks_by_source(&cfg, SourceKind::Chat, "slack:c-1").unwrap();

    assert_eq!(deleted, 1);
    assert!(target_path.exists());
    assert!(!link_path.exists());
}
