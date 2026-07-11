//! Unit tests for corrupt-DB recovery (`super`).

use super::super::connection::{clear_connection_cache, with_connection};
use super::super::db_path_for;
use super::recover_corrupt_db;
use crate::memory::config::MemoryConfig;
use anyhow::Context;
use tempfile::TempDir;

fn corrupt_test_config() -> (TempDir, MemoryConfig) {
    let tmp = TempDir::new().unwrap();
    let cfg = MemoryConfig::new(tmp.path());
    (tmp, cfg)
}

/// A malformed on-disk image must be quarantined (not deleted) and replaced by
/// a fresh, queryable schema so the store resumes.
#[test]
fn recover_corrupt_db_quarantines_and_rebuilds() {
    clear_connection_cache();
    let (_tmp, cfg) = corrupt_test_config();
    let db_path = db_path_for(&cfg);
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    std::fs::write(&db_path, b"this is not a sqlite database, it is garbage").unwrap();

    let recovered = recover_corrupt_db(&cfg).expect("recovery should succeed");
    assert!(recovered, "garbage image must be quarantined + rebuilt");

    let quarantined: Vec<_> = std::fs::read_dir(db_path.parent().unwrap())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .contains("chunks.db.corrupt-")
        })
        .collect();
    assert_eq!(
        quarantined.len(),
        1,
        "exactly one quarantined copy should exist"
    );

    clear_connection_cache();
    let count: i64 = with_connection(&cfg, |conn| {
        conn.query_row("SELECT COUNT(*) FROM mem_tree_jobs", [], |r| r.get(0))
            .context("count jobs")
    })
    .expect("rebuilt DB must be queryable");
    assert_eq!(count, 0, "rebuilt jobs table starts empty");
}

/// A legacy DB still in WAL mode can hold committed-but-uncheckpointed
/// transactions *only* in its `-wal` side-file. The stale-file cleanup path
/// (run on a cold-start I/O open error) must fold that data back into the main
/// DB — never delete the `-wal` — so committed rows survive a reopen.
#[test]
fn cleanup_preserves_committed_wal_data() {
    use super::{try_cleanup_stale_files, with_name_suffix};
    use rusqlite::Connection;

    let (_tmp, cfg) = corrupt_test_config();
    let db_path = db_path_for(&cfg);
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();

    // Build a WAL-mode DB whose committed data lives only in the `-wal`, then
    // copy the raw files to `db_path` *before* the connection closes (a close
    // would checkpoint the data into the main file and hide the bug). The
    // result is a WAL DB with uncheckpointed committed data and no live handle.
    let staging = TempDir::new().unwrap();
    let src_db = staging.path().join("src.db");
    {
        let conn = Connection::open(&src_db).unwrap();
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        conn.pragma_update(None, "wal_autocheckpoint", "0").unwrap();
        conn.execute("CREATE TABLE legacy (v TEXT)", []).unwrap();
        conn.execute("INSERT INTO legacy VALUES ('survives')", [])
            .unwrap();
        // Copy the main DB + WAL (not the rebuildable `-shm`) while `conn` still
        // holds them, i.e. before any checkpoint folds the WAL into the main
        // file. SQLite rebuilds the `-shm` from the `-wal` on the next open.
        for suffix in ["", "-wal"] {
            let s = with_name_suffix(&src_db, suffix);
            if s.exists() {
                std::fs::copy(&s, with_name_suffix(&db_path, suffix)).unwrap();
            }
        }
    }

    // Precondition: the committed row lives in the `-wal`, not the main file.
    let wal = with_name_suffix(&db_path, "-wal");
    assert!(
        wal.exists() && wal.metadata().unwrap().len() > 0,
        "committed data must live in a non-empty -wal for this test"
    );

    // Run the exact cleanup the connection retry path invokes on I/O errors.
    try_cleanup_stale_files(&db_path);

    // The committed row must survive: deleting the `-wal` would drop it.
    let conn = Connection::open(&db_path).unwrap();
    let v: String = conn
        .query_row("SELECT v FROM legacy", [], |r| r.get(0))
        .expect("committed WAL data must survive stale-file cleanup");
    assert_eq!(v, "survives");
}

/// A healthy DB must NOT be quarantined — `quick_check` passes, so good data is
/// preserved and recovery is a no-op returning `Ok(false)`.
#[test]
fn recover_corrupt_db_is_noop_on_healthy_db() {
    clear_connection_cache();
    let (_tmp, cfg) = corrupt_test_config();
    with_connection(&cfg, |conn| {
        conn.query_row("SELECT COUNT(*) FROM mem_tree_jobs", [], |r| {
            r.get::<_, i64>(0)
        })
        .context("seed healthy db")
    })
    .unwrap();

    let recovered = recover_corrupt_db(&cfg).expect("recovery should succeed");
    assert!(!recovered, "healthy DB must not be quarantined");

    let db_path = db_path_for(&cfg);
    let quarantined = std::fs::read_dir(db_path.parent().unwrap())
        .unwrap()
        .filter_map(|e| e.ok())
        .any(|e| e.file_name().to_string_lossy().contains(".corrupt-"));
    assert!(!quarantined, "no quarantine file should be created");
}
