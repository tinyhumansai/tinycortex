use super::*;
use crate::memory::config::MemoryConfig;
use crate::memory::queue::gate::DEFAULT_LLM_PERMITS;
use crate::memory::queue::handlers::ReembedProgress;
use crate::memory::queue::store::DEFAULT_LOCK_DURATION_MS;
use crate::memory::queue::store::{count_by_status, enqueue, get_job};
use crate::memory::queue::test_support::RecordingDelegates;
use crate::memory::queue::types::{FlushStalePayload, JobStatus, NewJob, ReembedBackfillPayload};
use tempfile::TempDir;

fn test_config() -> (TempDir, MemoryConfig) {
    let tmp = TempDir::new().unwrap();
    let cfg = MemoryConfig::new(tmp.path());
    (tmp, cfg)
}

fn sqlite_failure(code: rusqlite::ErrorCode, extended: i32, msg: &str) -> anyhow::Error {
    anyhow::Error::from(rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error {
            code,
            extended_code: extended,
        },
        Some(msg.into()),
    ))
}

#[tokio::test]
async fn run_once_returns_false_when_queue_is_empty() {
    let (_tmp, cfg) = test_config();
    let d = RecordingDelegates::admitting();
    assert!(!run_once(&cfg, &d).await.unwrap());
}

#[tokio::test]
async fn run_once_parks_unparseable_payload_as_unrecoverable() {
    use crate::memory::queue::scheduler;
    use crate::memory::queue::types::JobKind;

    let (_tmp, cfg) = test_config();
    let d = RecordingDelegates::admitting();

    // A poison job: its payload can never deserialize for its kind, so retrying
    // the identical bytes is hopeless. `max_attempts = 5` proves the fix parks
    // it on the FIRST failure rather than burning the retry budget. FlushStale is
    // deliberately not LLM-bound, so this test never touches the shared LLM gate.
    let poison = NewJob {
        kind: JobKind::FlushStale,
        payload_json: "{ this is not valid json".into(),
        dedupe_key: None,
        available_at_ms: None,
        max_attempts: Some(5),
    };
    let id = enqueue(&cfg, &poison).unwrap().expect("enqueued");

    // One worker step: claim → parse fails → settle.
    assert!(run_once(&cfg, &d).await.unwrap());

    let job = get_job(&cfg, &id).unwrap().unwrap();
    assert_eq!(
        job.status,
        JobStatus::Failed,
        "poison job must fail fast, not retry"
    );
    assert_eq!(job.failure_class.as_deref(), Some("unrecoverable"));

    // self_heal (requeue_transient_failed) must NOT resurrect it — that was the
    // infinite-requeue-every-tick bug (QI-2).
    let requeued = scheduler::self_heal(&cfg).unwrap();
    assert_eq!(requeued, 0, "unrecoverable poison job must stay parked");
    assert_eq!(
        get_job(&cfg, &id).unwrap().unwrap().status,
        JobStatus::Failed
    );
}

#[tokio::test]
async fn run_once_claims_and_completes_a_flush_stale_job() {
    let (_tmp, cfg) = test_config();
    let d = RecordingDelegates::admitting();
    let new_job = NewJob::flush_stale(&FlushStalePayload::default(), "2026-05-24", 3).unwrap();
    let id = enqueue(&cfg, &new_job).unwrap().expect("enqueued");

    assert!(run_once(&cfg, &d).await.unwrap());
    let job = get_job(&cfg, &id).unwrap().unwrap();
    assert_eq!(job.status, JobStatus::Done);
    assert!(job.completed_at_ms.is_some());
    assert!(job.locked_until_ms.is_none());
    assert_eq!(count_by_status(&cfg, JobStatus::Done).unwrap(), 1);
}

#[tokio::test]
async fn run_once_reschedules_reembed_jobs_that_defer() {
    let (_tmp, cfg) = test_config();
    let mut d = RecordingDelegates::admitting();
    d.signature = "provider=test;model=x;dims=3".into();
    *d.reembed.lock() =
        std::collections::VecDeque::from([ReembedProgress::Wrote { more_pending: true }]);

    let new_job = NewJob::reembed_backfill(&ReembedBackfillPayload {
        signature: d.signature.clone(),
    })
    .unwrap();
    let id = enqueue(&cfg, &new_job).unwrap().expect("enqueued");

    assert!(run_once(&cfg, &d).await.unwrap());
    let job = get_job(&cfg, &id).unwrap().unwrap();
    assert_eq!(job.status, JobStatus::Ready);
    assert_eq!(job.attempts, 0, "defer reverts the claim attempt bump");
    assert!(job.started_at_ms.is_none());
    assert!(job.locked_until_ms.is_none());
    assert!(job.available_at_ms > chrono::Utc::now().timestamp_millis());
}

#[test]
fn terminal_reembed_failure_clears_process_backfill_flag() {
    let (_tmp, cfg) = test_config();
    let mut new_job = NewJob::reembed_backfill(&ReembedBackfillPayload {
        signature: "provider=test;model=terminal;dims=3".into(),
    })
    .unwrap();
    new_job.max_attempts = Some(1);
    enqueue(&cfg, &new_job).unwrap().unwrap();
    let claimed = claim_next(&cfg, DEFAULT_LOCK_DURATION_MS).unwrap().unwrap();

    crate::memory::queue::ops::set_backfill_in_progress(true);
    settle_job(&cfg, &claimed, Err(anyhow::anyhow!("provider unavailable"))).unwrap();

    assert_eq!(
        get_job(&cfg, &claimed.id).unwrap().unwrap().status,
        JobStatus::Failed
    );
    assert!(!crate::memory::queue::ops::backfill_in_progress());
}

#[test]
fn completed_seal_rearms_same_level_when_live_buffer_still_crosses_gate() {
    use crate::memory::chunks::with_connection;
    use crate::memory::queue::types::SealPayload;

    let (_tmp, mut cfg) = test_config();
    cfg.tree.input_token_budget = 10;
    let tree = crate::memory::tree::get_or_create_tree(
        &cfg,
        crate::memory::tree::TreeKind::Source,
        "queue:edge",
    )
    .unwrap();
    let payload = SealPayload {
        tree_id: tree.id,
        level: 0,
        force_now_ms: None,
    };
    let original_id = enqueue(&cfg, &NewJob::seal(&payload).unwrap())
        .unwrap()
        .unwrap();
    let claimed = claim_next(&cfg, DEFAULT_LOCK_DURATION_MS).unwrap().unwrap();

    // Models content appended after the handler took and consumed its own
    // snapshot, while this job's active dedupe key still suppressed producers.
    with_connection(&cfg, |conn| {
        conn.execute(
            "INSERT INTO mem_tree_buffers
                (tree_id, level, item_ids_json, token_sum, oldest_at_ms, updated_at_ms)
             VALUES (?1, 0, '[\"late-chunk\"]', 10, 1, 1)",
            rusqlite::params![payload.tree_id],
        )?;
        Ok(())
    })
    .unwrap();

    settle_job(&cfg, &claimed, Ok(JobOutcome::Done)).unwrap();

    assert_eq!(
        get_job(&cfg, &original_id).unwrap().unwrap().status,
        JobStatus::Done
    );
    let replacement = claim_next(&cfg, DEFAULT_LOCK_DURATION_MS)
        .unwrap()
        .expect("same-level seal edge restored");
    assert_eq!(replacement.kind, crate::memory::queue::types::JobKind::Seal);
    assert_ne!(replacement.id, original_id);
}

#[tokio::test]
async fn run_once_holds_an_llm_permit_for_llm_bound_jobs() {
    // While a single worker is sequential, the gate must still be acquired and
    // released around the llm-bound handler so it ends free.
    let (_tmp, cfg) = test_config();
    let d = RecordingDelegates::admitting();
    let new_job = NewJob::reembed_backfill(&ReembedBackfillPayload {
        signature: "sig".into(),
    })
    .unwrap();
    enqueue(&cfg, &new_job).unwrap().unwrap();
    assert!(run_once(&cfg, &d).await.unwrap());
    assert_eq!(
        llm_gate().available_permits(),
        DEFAULT_LLM_PERMITS,
        "permit released after the llm-bound handler"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn run_once_defers_instead_of_blocking_when_llm_gate_is_full() {
    let (_tmp, cfg) = test_config();
    let d = RecordingDelegates::admitting();
    let gate = crate::memory::queue::gate::LlmGate::new(1);
    let held = gate.try_acquire().unwrap();
    let new_job = NewJob::reembed_backfill(&ReembedBackfillPayload {
        signature: "sig".into(),
    })
    .unwrap();
    let id = enqueue(&cfg, &new_job).unwrap().unwrap();

    // This would deadlock a current-thread runtime if the worker used the
    // gate's blocking acquire path.
    assert!(run_once_with_gate(&cfg, &d, &gate).await.unwrap());
    let job = get_job(&cfg, &id).unwrap().unwrap();
    assert_eq!(job.status, JobStatus::Ready);
    assert_eq!(job.attempts, 0);
    assert_eq!(gate.available_permits(), 0);
    drop(held);
}

#[test]
fn bootstrap_purges_retired_and_recovers_locks() {
    use crate::memory::chunks::with_connection;
    use rusqlite::params;
    let (_tmp, cfg) = test_config();
    with_connection(&cfg, |conn| {
        conn.execute(
            "INSERT INTO mem_tree_jobs (id, kind, payload_json, status, attempts,
                max_attempts, available_at_ms, created_at_ms)
             VALUES ('job:r', 'topic_route', '{}', 'ready', 0, 5, 0, 0)",
            params![],
        )?;
        Ok(())
    })
    .unwrap();
    let (purged, _recovered) = bootstrap(&cfg).unwrap();
    assert_eq!(purged, 1);
}

// ── SQLite error classifiers (ported verbatim) ───────────────────────────────

#[test]
fn is_sqlite_busy_matches_busy_and_locked() {
    assert!(is_sqlite_busy(&sqlite_failure(
        rusqlite::ErrorCode::DatabaseBusy,
        5,
        "database is locked"
    )));
    assert!(is_sqlite_busy(&sqlite_failure(
        rusqlite::ErrorCode::DatabaseLocked,
        6,
        "database table is locked"
    )));
}

#[test]
fn is_sqlite_busy_matches_through_context_and_text() {
    let wrapped = sqlite_failure(rusqlite::ErrorCode::DatabaseBusy, 5, "database is locked")
        .context("Failed to claim next mem_tree_jobs row")
        .context("with_connection closure failed");
    assert!(is_sqlite_busy(&wrapped));
    assert!(is_sqlite_busy(&anyhow::anyhow!(
        "Failed to claim next mem_tree_jobs row: database is locked"
    )));
}

#[test]
fn is_sqlite_busy_negatives() {
    assert!(!is_sqlite_busy(&sqlite_failure(
        rusqlite::ErrorCode::ConstraintViolation,
        19,
        "UNIQUE constraint failed"
    )));
    assert!(!is_sqlite_busy(&anyhow::anyhow!("upstream 500")));
}

#[test]
fn is_sqlite_io_transient_matches_family() {
    assert!(is_sqlite_io_transient(&sqlite_failure(
        rusqlite::ErrorCode::SystemIoFailure,
        1546,
        "disk I/O error"
    )));
    for ext in [4618, 4874, 5386, 8714] {
        assert!(is_sqlite_io_transient(&sqlite_failure(
            rusqlite::ErrorCode::SystemIoFailure,
            ext,
            "sqlite io failure"
        )));
    }
    assert!(is_sqlite_io_transient(&sqlite_failure(
        rusqlite::ErrorCode::CannotOpen,
        14,
        "unable to open database file"
    )));
    assert!(is_sqlite_io_transient(&anyhow::anyhow!(
        "memory_tree_db circuit breaker open: too many init failures"
    )));
    assert!(!is_sqlite_io_transient(&sqlite_failure(
        rusqlite::ErrorCode::ConstraintViolation,
        19,
        "UNIQUE constraint failed"
    )));
}

#[test]
fn is_sqlite_disk_full_matches_code_context_text() {
    assert!(is_sqlite_disk_full(&sqlite_failure(
        rusqlite::ErrorCode::DiskFull,
        13,
        "database or disk is full"
    )));
    let wrapped = sqlite_failure(
        rusqlite::ErrorCode::DiskFull,
        13,
        "database or disk is full",
    )
    .context("Failed to claim next mem_tree_jobs row");
    assert!(is_sqlite_disk_full(&wrapped));
    assert!(is_sqlite_disk_full(&anyhow::anyhow!(
        "row: database or disk is full: Error code 13: Insertion failed because database is full"
    )));
    assert!(!is_sqlite_disk_full(&sqlite_failure(
        rusqlite::ErrorCode::DatabaseBusy,
        5,
        "database is locked"
    )));
}

#[test]
fn is_sqlite_corrupt_matches_code_notadb_context_text() {
    assert!(is_sqlite_corrupt(&sqlite_failure(
        rusqlite::ErrorCode::DatabaseCorrupt,
        11,
        "database disk image is malformed"
    )));
    assert!(is_sqlite_corrupt(&sqlite_failure(
        rusqlite::ErrorCode::NotADatabase,
        26,
        "file is not a database"
    )));
    let wrapped = sqlite_failure(
        rusqlite::ErrorCode::DatabaseCorrupt,
        11,
        "database disk image is malformed",
    )
    .context("Failed to claim next mem_tree_jobs row");
    assert!(is_sqlite_corrupt(&wrapped));
    assert!(is_sqlite_corrupt(&anyhow::anyhow!(
        "row: database disk image is malformed"
    )));
    assert!(!is_sqlite_corrupt(&sqlite_failure(
        rusqlite::ErrorCode::DiskFull,
        13,
        "database or disk is full"
    )));
}

/// EIO (`5`), ENOSPC (`28`), and EROFS (`30`) are the persistent, user-only-
/// fixable host-FS family: a `std::io::Error` bubbling out of a filesystem call
/// classifies as host I/O whether it arrives typed, through anyhow context
/// layers, or flattened to its `(os error N)` text (Sentry CORE-RUST-19J).
#[test]
fn is_host_io_error_matches_family_code_context_text() {
    for code in [5, 28, 30] {
        let err = anyhow::Error::from(std::io::Error::from_raw_os_error(code));
        assert!(
            is_host_io_error(&err),
            "os error {code} must classify as host I/O"
        );
    }
    // The production shape: an io::Error wrapped in .with_context() twice; the
    // downcast must still find it through the anyhow context chain.
    let wrapped = anyhow::Error::from(std::io::Error::from_raw_os_error(5))
        .context("Failed to create memory_tree dir: /home/x/workspace/memory_tree")
        .context("with_connection closure failed");
    assert!(is_host_io_error(&wrapped));
    // Text fallback: no io::Error to downcast (flattened to a plain string), the
    // os-error-number anchor still classifies it.
    assert!(is_host_io_error(&anyhow::anyhow!(
        "Failed to create memory_tree dir: /home/x/workspace/memory_tree: \
         Input/output error (os error 5)"
    )));
}

/// EACCES (`13`, a permission bug), ENOENT (`2`), `SQLITE_FULL` (its own arm),
/// and unrelated errors must NOT be swallowed as host I/O — they are real bugs
/// or handled elsewhere and must keep reporting.
#[test]
fn is_host_io_error_negatives() {
    assert!(!is_host_io_error(&anyhow::Error::from(
        std::io::Error::from_raw_os_error(13)
    )));
    assert!(!is_host_io_error(&anyhow::Error::from(
        std::io::Error::from_raw_os_error(2)
    )));
    // SQLITE_FULL stays in is_sqlite_disk_full's arm, not here.
    assert!(!is_host_io_error(&sqlite_failure(
        rusqlite::ErrorCode::DiskFull,
        13,
        "database or disk is full"
    )));
    assert!(!is_host_io_error(&anyhow::anyhow!(
        "upstream returned 500: internal server error"
    )));
}
