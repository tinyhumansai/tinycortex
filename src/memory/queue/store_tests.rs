use super::*;
use crate::memory::chunks::with_connection;
use crate::memory::config::MemoryConfig;
use crate::memory::queue::types::ExtractChunkPayload;
use tempfile::TempDir;

fn test_config() -> (TempDir, MemoryConfig) {
    let tmp = TempDir::new().unwrap();
    let cfg = MemoryConfig::new(tmp.path());
    (tmp, cfg)
}

#[test]
fn enqueue_and_claim_roundtrip() {
    let (_tmp, cfg) = test_config();
    let nj = NewJob::extract_chunk(&ExtractChunkPayload {
        chunk_id: "c1".into(),
    })
    .unwrap();
    let id = enqueue(&cfg, &nj).unwrap().expect("inserted");

    let claimed = claim_next(&cfg, DEFAULT_LOCK_DURATION_MS).unwrap().unwrap();
    assert_eq!(claimed.id, id);
    assert_eq!(claimed.status, JobStatus::Running);
    assert_eq!(claimed.attempts, 1);
    assert!(claimed.locked_until_ms.is_some());

    // Second claim should see no eligible row (the only one is now running).
    let again = claim_next(&cfg, DEFAULT_LOCK_DURATION_MS).unwrap();
    assert!(again.is_none());
}

#[test]
fn typed_failure_columns_roundtrip_as_none_by_default() {
    let (_tmp, cfg) = test_config();
    let nj = NewJob::extract_chunk(&ExtractChunkPayload {
        chunk_id: "c-typed".into(),
    })
    .unwrap();
    let id = enqueue(&cfg, &nj).unwrap().expect("inserted");

    let claimed = claim_next(&cfg, DEFAULT_LOCK_DURATION_MS).unwrap().unwrap();
    assert_eq!(claimed.failure_reason, None);
    assert_eq!(claimed.failure_class, None);

    let row = get_job(&cfg, &id).unwrap().unwrap();
    assert_eq!(row.failure_reason, None);
    assert_eq!(row.failure_class, None);
}

#[test]
fn enqueue_dedupes_active_jobs() {
    let (_tmp, cfg) = test_config();
    let nj = NewJob::extract_chunk(&ExtractChunkPayload {
        chunk_id: "c1".into(),
    })
    .unwrap();
    let id1 = enqueue(&cfg, &nj).unwrap();
    let id2 = enqueue(&cfg, &nj).unwrap();
    assert!(id1.is_some());
    assert!(id2.is_none(), "duplicate should be suppressed while ready");
    assert_eq!(count_total(&cfg).unwrap(), 1);
}

#[test]
fn enqueue_after_done_creates_fresh_row() {
    use crate::memory::queue::store_settle::mark_done;
    let (_tmp, cfg) = test_config();
    let nj = NewJob::extract_chunk(&ExtractChunkPayload {
        chunk_id: "c1".into(),
    })
    .unwrap();
    let id1 = enqueue(&cfg, &nj).unwrap().unwrap();
    let claimed = claim_next(&cfg, DEFAULT_LOCK_DURATION_MS).unwrap().unwrap();
    assert_eq!(claimed.id, id1);
    mark_done(&cfg, &claimed).unwrap();

    // The dedupe key is free now (the partial index excludes 'done').
    let id2 = enqueue(&cfg, &nj).unwrap();
    assert!(id2.is_some());
    assert_ne!(id2.unwrap(), id1);
    assert_eq!(count_total(&cfg).unwrap(), 2);
}

#[test]
fn count_by_status_reports_each_state() {
    use crate::memory::queue::store_settle::mark_done;
    let (_tmp, cfg) = test_config();
    for i in 0..3 {
        let nj = NewJob::extract_chunk(&ExtractChunkPayload {
            chunk_id: format!("c{i}"),
        })
        .unwrap();
        enqueue(&cfg, &nj).unwrap();
    }
    assert_eq!(count_by_status(&cfg, JobStatus::Ready).unwrap(), 3);
    let claimed = claim_next(&cfg, DEFAULT_LOCK_DURATION_MS).unwrap().unwrap();
    mark_done(&cfg, &claimed).unwrap();
    assert_eq!(count_by_status(&cfg, JobStatus::Done).unwrap(), 1);
    assert_eq!(count_by_status(&cfg, JobStatus::Ready).unwrap(), 2);
}

#[test]
fn backoff_grows_then_caps() {
    assert_eq!(backoff_ms(1), 60_000);
    assert_eq!(backoff_ms(2), 120_000);
    assert_eq!(backoff_ms(3), 240_000);
    assert_eq!(backoff_ms(20), RETRY_CAP_MS);
    assert_eq!(backoff_ms(99), RETRY_CAP_MS);
}

/// Retired-kind tolerance: a leftover `topic_route` / `digest_daily` row (from
/// before the global/topic trees were removed) must NOT be claimed (which would
/// crash `row_to_job` on the unknown kind), and `purge_retired_jobs` removes it
/// while leaving live rows untouched.
#[test]
fn retired_kind_rows_are_skipped_then_purged() {
    let (_tmp, cfg) = test_config();

    // Insert two raw retired rows directly (no NewJob path exists for them).
    with_connection(&cfg, |conn| {
        for (id, kind) in [
            ("job:retired-1", "topic_route"),
            ("job:retired-2", "digest_daily"),
        ] {
            conn.execute(
                "INSERT INTO mem_tree_jobs (id, kind, payload_json, status, attempts,
                    max_attempts, available_at_ms, created_at_ms)
                 VALUES (?1, ?2, '{}', 'ready', 0, 5, 0, 0)",
                params![id, kind],
            )?;
        }
        Ok(())
    })
    .unwrap();

    // A live job alongside them.
    let live = NewJob::extract_chunk(&ExtractChunkPayload {
        chunk_id: "live".into(),
    })
    .unwrap();
    let live_id = enqueue(&cfg, &live).unwrap().unwrap();

    // claim_next must pick the live row and never crash on the retired ones.
    let claimed = claim_next(&cfg, DEFAULT_LOCK_DURATION_MS).unwrap().unwrap();
    assert_eq!(claimed.id, live_id);
    assert_eq!(claimed.kind, JobKind::ExtractChunk);
    // No further claimable rows (retired ones are excluded).
    assert!(claim_next(&cfg, DEFAULT_LOCK_DURATION_MS)
        .unwrap()
        .is_none());

    // Purge removes exactly the two retired rows; the live row stays.
    let purged = purge_retired_jobs(&cfg).unwrap();
    assert_eq!(purged, 2);
    assert_eq!(count_total(&cfg).unwrap(), 1);
    assert!(get_job(&cfg, &live_id).unwrap().is_some());
}

#[test]
fn is_retired_kind_recognises_legacy_strings() {
    assert!(is_retired_kind("topic_route"));
    assert!(is_retired_kind("digest_daily"));
    assert!(!is_retired_kind("extract_chunk"));
    assert!(!is_retired_kind("seal"));
}
