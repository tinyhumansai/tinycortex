use super::*;
use crate::memory::chunks::with_connection;
use crate::memory::config::MemoryConfig;
use crate::memory::queue::test_support::RecordingDelegates;
use rusqlite::params;
use tempfile::TempDir;

fn test_config() -> (TempDir, MemoryConfig) {
    let tmp = TempDir::new().unwrap();
    let cfg = MemoryConfig::new(tmp.path());
    (tmp, cfg)
}

fn count_reembed_jobs(cfg: &MemoryConfig) -> u64 {
    with_connection(cfg, |conn| {
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM mem_tree_jobs WHERE kind = 'reembed_backfill'",
            params![],
            |r| r.get(0),
        )?;
        Ok(n.max(0) as u64)
    })
    .unwrap()
}

#[test]
fn backfill_flag_setters_are_wired() {
    // The flag is process-global and shared across parallel tests, so this only
    // exercises the setters compile/run path (it does not assert the value,
    // which other tests may concurrently toggle).
    set_backfill_in_progress(true);
    let _ = backfill_in_progress();
    set_backfill_in_progress(false);
}

#[test]
fn ensure_reembed_backfill_enqueues_only_when_uncovered() {
    // Covered space → no chain.
    let (_t0, covered) = test_config();
    let mut d = RecordingDelegates::admitting();
    d.uncovered = false;
    ensure_reembed_backfill(&covered, &d).unwrap();
    assert_eq!(count_reembed_jobs(&covered), 0);

    // Uncovered space → exactly one chain, idempotent on re-call (per-signature
    // dedupe key).
    let (_t1, cfg) = test_config();
    let mut d2 = RecordingDelegates::admitting();
    d2.uncovered = true;
    ensure_reembed_backfill(&cfg, &d2).unwrap();
    assert_eq!(count_reembed_jobs(&cfg), 1);
    ensure_reembed_backfill(&cfg, &d2).unwrap();
    assert_eq!(
        count_reembed_jobs(&cfg),
        1,
        "re-call dedupes to a single chain per signature"
    );
}
