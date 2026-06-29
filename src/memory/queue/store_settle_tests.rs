use super::*;
use crate::memory::chunks::with_connection;
use crate::memory::config::MemoryConfig;
use crate::memory::queue::store::{
    claim_next, count_by_status, count_failed_unrecoverable, count_total, enqueue, get_job,
    DEFAULT_LOCK_DURATION_MS,
};
use crate::memory::queue::types::{
    AppendBufferPayload, AppendTarget, ExtractChunkPayload, JobStatus, NewJob, NodeRef,
};
use tempfile::TempDir;

fn test_config() -> (TempDir, MemoryConfig) {
    let tmp = TempDir::new().unwrap();
    let cfg = MemoryConfig::new(tmp.path());
    (tmp, cfg)
}

fn extract_job(chunk: &str, max_attempts: u32) -> NewJob {
    let mut nj = NewJob::extract_chunk(&ExtractChunkPayload {
        chunk_id: chunk.into(),
    })
    .unwrap();
    nj.max_attempts = Some(max_attempts);
    nj
}

#[test]
fn mark_done_settles_current_lessee() {
    let (_tmp, cfg) = test_config();
    let id = enqueue(&cfg, &extract_job("c-happy", 5))
        .unwrap()
        .expect("inserted");
    let claimed = claim_next(&cfg, DEFAULT_LOCK_DURATION_MS).unwrap().unwrap();
    mark_done(&cfg, &claimed).unwrap();
    let row = get_job(&cfg, &id).unwrap().unwrap();
    assert_eq!(row.status, JobStatus::Done);
    assert!(row.completed_at_ms.is_some());
    assert!(row.locked_until_ms.is_none());
}

#[test]
fn mark_failed_typed_unrecoverable_terminates_immediately() {
    let (_tmp, cfg) = test_config();
    let id = enqueue(&cfg, &extract_job("c-budget", 5))
        .unwrap()
        .expect("inserted");
    let claimed = claim_next(&cfg, DEFAULT_LOCK_DURATION_MS).unwrap().unwrap();
    assert_eq!(claimed.attempts, 1, "first claim");
    let failure = JobFailure::budget_exhausted();
    mark_failed_typed(&cfg, &claimed, "Insufficient budget", Some(&failure)).unwrap();

    let row = get_job(&cfg, &id).unwrap().unwrap();
    assert_eq!(row.status, JobStatus::Failed);
    assert!(row.completed_at_ms.is_some());
    assert_eq!(row.failure_reason.as_deref(), Some("budget_exhausted"));
    assert_eq!(row.failure_class.as_deref(), Some("unrecoverable"));
    assert_eq!(row.last_error.as_deref(), Some("Insufficient budget"));
}

#[test]
fn mark_failed_typed_transient_still_retries() {
    let (_tmp, cfg) = test_config();
    let id = enqueue(&cfg, &extract_job("c-transient", 5))
        .unwrap()
        .expect("inserted");
    let claimed = claim_next(&cfg, DEFAULT_LOCK_DURATION_MS).unwrap().unwrap();
    let failure = JobFailure::transient("upstream_503");
    mark_failed_typed(&cfg, &claimed, "503 upstream", Some(&failure)).unwrap();

    let row = get_job(&cfg, &id).unwrap().unwrap();
    assert_eq!(row.status, JobStatus::Ready, "transient must retry");
    assert!(row.available_at_ms > Utc::now().timestamp_millis());
    assert_eq!(row.failure_reason, None, "typed cols unset on retry");
    assert_eq!(row.failure_class, None);
}

#[test]
fn count_failed_unrecoverable_excludes_transient_and_untyped() {
    let (_tmp, cfg) = test_config();

    let fail_one = |chunk: &str, max_attempts: u32, failure: Option<&JobFailure>| {
        enqueue(&cfg, &extract_job(chunk, max_attempts))
            .unwrap()
            .expect("inserted");
        let claimed = claim_next(&cfg, DEFAULT_LOCK_DURATION_MS).unwrap().unwrap();
        mark_failed_typed(&cfg, &claimed, "boom", failure).unwrap();
    };

    fail_one("c-unrec", 5, Some(&JobFailure::budget_exhausted()));
    fail_one("c-trans", 1, Some(&JobFailure::transient("net")));
    fail_one("c-null", 1, None);

    assert_eq!(count_by_status(&cfg, JobStatus::Failed).unwrap(), 3);
    assert_eq!(count_failed_unrecoverable(&cfg).unwrap(), 1);
}

#[test]
fn mark_failed_retries_then_terminates() {
    let (_tmp, cfg) = test_config();
    let payload = AppendBufferPayload {
        node: NodeRef::Leaf {
            chunk_id: "c1".into(),
        },
        target: AppendTarget::Source {
            source_id: "slack:#x".into(),
        },
    };
    let mut nj = NewJob::append_buffer(&payload).unwrap();
    nj.max_attempts = Some(2);
    let id = enqueue(&cfg, &nj).unwrap().unwrap();

    let attempt1 = claim_next(&cfg, DEFAULT_LOCK_DURATION_MS).unwrap().unwrap();
    mark_failed(&cfg, &attempt1, "boom").unwrap();
    let row = get_job(&cfg, &id).unwrap().unwrap();
    assert_eq!(row.status, JobStatus::Ready);
    assert!(row.available_at_ms > Utc::now().timestamp_millis());
    assert_eq!(row.last_error.as_deref(), Some("boom"));

    // Force the row available again so the test doesn't hinge on sleep.
    with_connection(&cfg, |c| {
        c.execute(
            "UPDATE mem_tree_jobs SET available_at_ms = 0 WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    })
    .unwrap();

    let attempt2 = claim_next(&cfg, DEFAULT_LOCK_DURATION_MS).unwrap().unwrap();
    mark_failed(&cfg, &attempt2, "fatal").unwrap();
    let row = get_job(&cfg, &id).unwrap().unwrap();
    assert_eq!(row.status, JobStatus::Failed);
    assert_eq!(row.last_error.as_deref(), Some("fatal"));
    assert!(row.completed_at_ms.is_some());
}

#[test]
fn mark_failed_persists_full_error_unredacted() {
    let (_tmp, cfg) = test_config();
    let mut nj = extract_job("c-raw", 1);
    nj.max_attempts = Some(1);
    let id = enqueue(&cfg, &nj).unwrap().unwrap();
    let claim = claim_next(&cfg, DEFAULT_LOCK_DURATION_MS).unwrap().unwrap();
    let raw = "upstream returned 401: Bearer abc123.def-456 token rejected";
    mark_failed(&cfg, &claim, raw).unwrap();
    let row = get_job(&cfg, &id).unwrap().unwrap();
    assert_eq!(row.status, JobStatus::Failed);
    // The persisted column keeps the full original — scrubbing is a log concern.
    assert_eq!(row.last_error.as_deref(), Some(raw));
}

#[test]
fn requeue_failed_resets_failed_jobs_only() {
    let (_tmp, cfg) = test_config();
    let id_a = enqueue(&cfg, &extract_job("a", 1)).unwrap().unwrap();
    let claim_a = claim_next(&cfg, DEFAULT_LOCK_DURATION_MS).unwrap().unwrap();
    mark_failed_typed(
        &cfg,
        &claim_a,
        "Insufficient budget",
        Some(&JobFailure::budget_exhausted()),
    )
    .unwrap();
    assert_eq!(
        get_job(&cfg, &id_a).unwrap().unwrap().status,
        JobStatus::Failed
    );

    let id_b = enqueue(&cfg, &extract_job("b", 5)).unwrap().unwrap();

    let requeued = requeue_failed(&cfg).unwrap();
    assert_eq!(requeued, 1);

    let row_a = get_job(&cfg, &id_a).unwrap().unwrap();
    assert_eq!(row_a.status, JobStatus::Ready);
    assert_eq!(row_a.attempts, 0);
    assert_eq!(row_a.failure_reason, None);
    assert_eq!(row_a.failure_class, None);
    assert_eq!(row_a.last_error, None);
    assert!(row_a.completed_at_ms.is_none());

    assert_eq!(
        get_job(&cfg, &id_b).unwrap().unwrap().status,
        JobStatus::Ready
    );
}

#[test]
fn requeue_transient_failed_skips_unrecoverable_jobs() {
    let (_tmp, cfg) = test_config();
    // A: terminal via exhausted retry budget, no typed class.
    let id_a = enqueue(&cfg, &extract_job("a-transient", 1))
        .unwrap()
        .unwrap();
    let claim_a = claim_next(&cfg, DEFAULT_LOCK_DURATION_MS).unwrap().unwrap();
    mark_failed(&cfg, &claim_a, "connection reset by peer").unwrap();

    // B: terminal with an unrecoverable classification.
    let id_b = enqueue(&cfg, &extract_job("b-unrec", 1)).unwrap().unwrap();
    let claim_b = claim_next(&cfg, DEFAULT_LOCK_DURATION_MS).unwrap().unwrap();
    mark_failed_typed(
        &cfg,
        &claim_b,
        "Insufficient budget",
        Some(&JobFailure::budget_exhausted()),
    )
    .unwrap();

    let requeued = requeue_transient_failed(&cfg).unwrap();
    assert_eq!(requeued, 1, "only the unclassified failure requeues");

    assert_eq!(
        get_job(&cfg, &id_a).unwrap().unwrap().status,
        JobStatus::Ready
    );
    let row_b = get_job(&cfg, &id_b).unwrap().unwrap();
    assert_eq!(row_b.status, JobStatus::Failed);
    assert_eq!(row_b.failure_class.as_deref(), Some("unrecoverable"));
}

#[test]
fn recover_stale_locks_resets_running_rows() {
    let (_tmp, cfg) = test_config();
    let id = enqueue(&cfg, &extract_job("c1", 5)).unwrap().unwrap();
    // Claim with a lock window already in the past so recovery sees it expired.
    let _ = claim_next(&cfg, -1).unwrap().unwrap();
    let recovered = recover_stale_locks(&cfg).unwrap();
    assert_eq!(recovered, 1);
    assert_eq!(
        get_job(&cfg, &id).unwrap().unwrap().status,
        JobStatus::Ready
    );
}

#[test]
fn release_running_locks_resets_running_rows_regardless_of_lease() {
    let (_tmp, cfg) = test_config();
    let id = enqueue(&cfg, &extract_job("c-release", 5))
        .unwrap()
        .unwrap();
    // A long, still-valid lease: recover_stale_locks would NOT touch this.
    let _ = claim_next(&cfg, DEFAULT_LOCK_DURATION_MS).unwrap().unwrap();
    let released = release_running_locks(&cfg).unwrap();
    assert_eq!(released, 1);
    let row = get_job(&cfg, &id).unwrap().unwrap();
    assert_eq!(row.status, JobStatus::Ready);
    assert!(row.locked_until_ms.is_none());
}

#[test]
fn stale_worker_settlement_is_noop() {
    let (_tmp, cfg) = test_config();
    let id = enqueue(&cfg, &extract_job("c-stale", 5))
        .unwrap()
        .expect("inserted");

    // Worker A claims with an already-expired lock.
    let worker_a_job = claim_next(&cfg, -1).unwrap().unwrap();
    assert_eq!(worker_a_job.attempts, 1);
    // Lease expires; recover; Worker B re-claims (attempts=2).
    assert_eq!(recover_stale_locks(&cfg).unwrap(), 1);
    let worker_b_job = claim_next(&cfg, DEFAULT_LOCK_DURATION_MS).unwrap().unwrap();
    assert_eq!(worker_b_job.attempts, 2);

    // Worker A (stale) settles using its old snapshot — must be a no-op.
    mark_done(&cfg, &worker_a_job).unwrap();
    let row = get_job(&cfg, &id).unwrap().unwrap();
    assert_eq!(row.status, JobStatus::Running);
    assert_eq!(row.attempts, 2);
}

#[test]
fn stale_worker_mark_failed_is_noop() {
    let (_tmp, cfg) = test_config();
    let id = enqueue(&cfg, &extract_job("c-stale-fail", 5))
        .unwrap()
        .expect("inserted");
    let worker_a_job = claim_next(&cfg, -1).unwrap().unwrap();
    assert_eq!(recover_stale_locks(&cfg).unwrap(), 1);
    let worker_b_job = claim_next(&cfg, DEFAULT_LOCK_DURATION_MS).unwrap().unwrap();
    assert_eq!(worker_b_job.attempts, 2);

    mark_failed(&cfg, &worker_a_job, "stale error").unwrap();
    let row = get_job(&cfg, &id).unwrap().unwrap();
    assert_eq!(row.status, JobStatus::Running);
    assert_ne!(row.last_error.as_deref(), Some("stale error"));
    assert_eq!(row.attempts, 2);
}

#[test]
fn mark_deferred_does_not_increment_attempts() {
    let (_tmp, cfg) = test_config();
    let id = enqueue(&cfg, &extract_job("c-defer-1", 5))
        .unwrap()
        .expect("inserted");
    let claim1 = claim_next(&cfg, DEFAULT_LOCK_DURATION_MS).unwrap().unwrap();
    assert_eq!(claim1.attempts, 1, "first claim bumps attempts to 1");

    mark_deferred(&cfg, &claim1, 0, "rate_limited").unwrap();

    let row = get_job(&cfg, &id).unwrap().unwrap();
    assert_eq!(row.status, JobStatus::Ready);
    assert_eq!(row.attempts, 0, "Defer reverts the claim's attempts bump");
    assert_eq!(row.last_error.as_deref(), Some("rate_limited"));
    assert_eq!(row.available_at_ms, 0);
    assert!(row.locked_until_ms.is_none());
    assert!(row.started_at_ms.is_none());

    let claim2 = claim_next(&cfg, DEFAULT_LOCK_DURATION_MS).unwrap().unwrap();
    assert_eq!(claim2.id, id);
    assert_eq!(claim2.attempts, 1, "Defer didn't count toward the budget");
}

#[test]
fn deferred_row_not_claimable_until_until_ms() {
    let (_tmp, cfg) = test_config();
    let id = enqueue(&cfg, &extract_job("c-defer-2", 5))
        .unwrap()
        .expect("inserted");
    let claimed = claim_next(&cfg, DEFAULT_LOCK_DURATION_MS).unwrap().unwrap();
    let future_ms = Utc::now().timestamp_millis() + 60_000;
    mark_deferred(&cfg, &claimed, future_ms, "warming_up").unwrap();

    assert!(
        claim_next(&cfg, DEFAULT_LOCK_DURATION_MS)
            .unwrap()
            .is_none(),
        "deferred row must not be claimable before until_ms"
    );

    with_connection(&cfg, |c| {
        c.execute(
            "UPDATE mem_tree_jobs SET available_at_ms = 0 WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    })
    .unwrap();
    let claim2 = claim_next(&cfg, DEFAULT_LOCK_DURATION_MS).unwrap().unwrap();
    assert_eq!(claim2.id, id);
    assert_eq!(claim2.attempts, 1, "Defer left attempts at pre-claim 0");
}

#[test]
fn mixed_outcomes_stress() {
    let (_tmp, cfg) = test_config();
    let id_a = enqueue(&cfg, &extract_job("c-mix-a", 5)).unwrap().unwrap();
    let id_b = enqueue(&cfg, &extract_job("c-mix-b", 5)).unwrap().unwrap();
    let id_c = enqueue(&cfg, &extract_job("c-mix-c", 5)).unwrap().unwrap();

    let claim_a = claim_next(&cfg, DEFAULT_LOCK_DURATION_MS).unwrap().unwrap();
    let claim_b = claim_next(&cfg, DEFAULT_LOCK_DURATION_MS).unwrap().unwrap();
    let claim_c = claim_next(&cfg, DEFAULT_LOCK_DURATION_MS).unwrap().unwrap();
    let mut got: Vec<&str> = vec![&claim_a.id, &claim_b.id, &claim_c.id];
    got.sort();
    let mut want = vec![id_a.as_str(), id_b.as_str(), id_c.as_str()];
    want.sort();
    assert_eq!(got, want);

    let until_ms = Utc::now().timestamp_millis() + 30_000;
    mark_done(&cfg, &claim_a).unwrap();
    mark_failed(&cfg, &claim_b, "transient_error").unwrap();
    mark_deferred(&cfg, &claim_c, until_ms, "rate_limited").unwrap();

    let row_a = get_job(&cfg, &id_a).unwrap().unwrap();
    assert_eq!(row_a.status, JobStatus::Done);
    assert!(row_a.completed_at_ms.is_some());

    let row_b = get_job(&cfg, &id_b).unwrap().unwrap();
    assert_eq!(row_b.status, JobStatus::Ready);
    assert_eq!(row_b.attempts, 1, "Err keeps the claim's attempts bump");
    assert!(row_b.available_at_ms > Utc::now().timestamp_millis());

    let row_c = get_job(&cfg, &id_c).unwrap().unwrap();
    assert_eq!(row_c.status, JobStatus::Ready);
    assert_eq!(row_c.attempts, 0, "Defer reverts the claim's attempts bump");
    assert_eq!(row_c.available_at_ms, until_ms);
    assert!(row_c.started_at_ms.is_none());
}

#[test]
fn mark_deferred_stale_lease_is_noop() {
    let (_tmp, cfg) = test_config();
    let id = enqueue(&cfg, &extract_job("c-defer-stale", 5))
        .unwrap()
        .expect("inserted");
    let worker_a_job = claim_next(&cfg, -1).unwrap().unwrap();
    assert_eq!(recover_stale_locks(&cfg).unwrap(), 1);
    let worker_b_job = claim_next(&cfg, DEFAULT_LOCK_DURATION_MS).unwrap().unwrap();
    assert_eq!(worker_b_job.attempts, 2);

    let stale_until_ms = Utc::now().timestamp_millis() + 999_000;
    mark_deferred(&cfg, &worker_a_job, stale_until_ms, "stale_defer").unwrap();

    let row = get_job(&cfg, &id).unwrap().unwrap();
    assert_eq!(row.status, JobStatus::Running);
    assert_eq!(row.attempts, 2);
    assert_ne!(row.last_error.as_deref(), Some("stale_defer"));
    assert_ne!(row.available_at_ms, stale_until_ms);
    let _ = count_total(&cfg);
}
