use super::*;
use crate::memory::config::MemoryConfig;
use crate::memory::queue::store::{claim_next, count_by_status, DEFAULT_LOCK_DURATION_MS};
use crate::memory::queue::store_settle::mark_failed;
use crate::memory::queue::types::{ExtractChunkPayload, JobKind, JobStatus, NewJob};
use tempfile::TempDir;

fn test_config() -> (TempDir, MemoryConfig) {
    let tmp = TempDir::new().unwrap();
    let cfg = MemoryConfig::new(tmp.path());
    (tmp, cfg)
}

#[test]
fn enqueue_flush_stale_dedupes_within_current_block() {
    let (_tmp, cfg) = test_config();
    assert!(enqueue_flush_stale(&cfg).unwrap().is_some());
    assert!(
        enqueue_flush_stale(&cfg).unwrap().is_none(),
        "second enqueue in the same 3h block is dedupe-suppressed"
    );
    assert_eq!(count_by_status(&cfg, JobStatus::Ready).unwrap(), 1);

    let claimed = claim_next(&cfg, DEFAULT_LOCK_DURATION_MS).unwrap().unwrap();
    assert_eq!(claimed.kind, JobKind::FlushStale);
}

#[test]
fn self_heal_requeues_transient_failures() {
    let (_tmp, cfg) = test_config();
    let mut nj = NewJob::extract_chunk(&ExtractChunkPayload {
        chunk_id: "c1".into(),
    })
    .unwrap();
    nj.max_attempts = Some(1);
    crate::memory::queue::store::enqueue(&cfg, &nj)
        .unwrap()
        .unwrap();
    let claim = claim_next(&cfg, DEFAULT_LOCK_DURATION_MS).unwrap().unwrap();
    mark_failed(&cfg, &claim, "connection reset by peer").unwrap();
    assert_eq!(count_by_status(&cfg, JobStatus::Failed).unwrap(), 1);

    let requeued = self_heal(&cfg).unwrap();
    assert_eq!(requeued, 1);
    assert_eq!(count_by_status(&cfg, JobStatus::Ready).unwrap(), 1);
}
