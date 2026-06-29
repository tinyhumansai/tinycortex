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
