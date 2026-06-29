use super::*;
use crate::memory::chunks::with_connection;
use crate::memory::config::MemoryConfig;
use crate::memory::queue::handlers::{ExtractDecision, ReembedProgress};
use crate::memory::queue::store::{count_by_status, count_total, enqueue, purge_retired_jobs};
use crate::memory::queue::test_support::RecordingDelegates;
use crate::memory::queue::types::{ExtractChunkPayload, JobStatus, NewJob};
use rusqlite::params;
use tempfile::TempDir;

fn test_config() -> (TempDir, MemoryConfig) {
    let tmp = TempDir::new().unwrap();
    let cfg = MemoryConfig::new(tmp.path());
    (tmp, cfg)
}

#[tokio::test]
async fn drain_until_idle_is_noop_when_queue_is_empty() {
    let (_tmp, cfg) = test_config();
    let d = RecordingDelegates::admitting();
    drain_until_idle(&cfg, &d).await.unwrap();
}

/// The whole pipeline drains: one `extract_chunk` fans out to `append_buffer`
/// then `seal`, and all three settle as `done`.
#[tokio::test]
async fn drain_runs_the_full_extract_append_seal_pipeline() {
    let (_tmp, cfg) = test_config();
    let d = RecordingDelegates::admitting();

    let nj = NewJob::extract_chunk(&ExtractChunkPayload {
        chunk_id: "c1".into(),
    })
    .unwrap();
    enqueue(&cfg, &nj).unwrap().unwrap();

    drain_until_idle(&cfg, &d).await.unwrap();

    use std::sync::atomic::Ordering::Relaxed;
    assert_eq!(d.counts.extract.load(Relaxed), 1, "extracted once");
    assert_eq!(d.counts.append.load(Relaxed), 1, "appended once");
    assert_eq!(d.counts.seal.load(Relaxed), 1, "sealed once");
    // extract + append + seal all settled done; nothing left ready/running.
    assert_eq!(count_by_status(&cfg, JobStatus::Done).unwrap(), 3);
    assert_eq!(count_by_status(&cfg, JobStatus::Ready).unwrap(), 0);
}

/// A deferring job parks itself and the drain terminates (it is no longer
/// immediately claimable), without burning its retry budget.
#[tokio::test]
async fn drain_terminates_on_a_deferred_job() {
    use crate::memory::queue::types::ReembedBackfillPayload;
    let (_tmp, cfg) = test_config();
    let mut d = RecordingDelegates::admitting();
    *d.reembed.lock() =
        std::collections::VecDeque::from([ReembedProgress::Wrote { more_pending: true }]);

    let nj = NewJob::reembed_backfill(&ReembedBackfillPayload {
        signature: d.signature.clone(),
    })
    .unwrap();
    enqueue(&cfg, &nj).unwrap().unwrap();

    drain_until_idle(&cfg, &d).await.unwrap();
    // The reembed batch ran exactly once, then the deferred row parked.
    assert_eq!(
        d.counts.reembed.load(std::sync::atomic::Ordering::Relaxed),
        1
    );
    assert_eq!(count_by_status(&cfg, JobStatus::Ready).unwrap(), 1);
}

/// Retired-kind rows left in an old queue must not crash the drain: they are
/// skipped on claim, the live work still completes, and `purge_retired_jobs`
/// removes them afterwards.
#[tokio::test]
async fn drain_tolerates_retired_kind_rows() {
    let (_tmp, cfg) = test_config();
    let d = RecordingDelegates::admitting();

    with_connection(&cfg, |conn| {
        conn.execute(
            "INSERT INTO mem_tree_jobs (id, kind, payload_json, status, attempts,
                max_attempts, available_at_ms, created_at_ms)
             VALUES ('job:retired', 'digest_daily', '{}', 'ready', 0, 5, 0, 0)",
            params![],
        )?;
        Ok(())
    })
    .unwrap();

    // A live, terminal job (drops → no follow-ups) so the drain is bounded.
    let mut dd = RecordingDelegates::admitting();
    dd.extract = Some(ExtractDecision {
        kept: false,
        uses_document_subtree: false,
        tree_scope: "slack:#eng".into(),
    });
    let nj = NewJob::extract_chunk(&ExtractChunkPayload {
        chunk_id: "c1".into(),
    })
    .unwrap();
    enqueue(&cfg, &nj).unwrap().unwrap();

    // Drain must not panic on the retired row.
    drain_until_idle(&cfg, &dd).await.unwrap();
    assert_eq!(count_by_status(&cfg, JobStatus::Done).unwrap(), 1);

    // The retired row is still present (skipped, not processed) until purged.
    assert_eq!(count_total(&cfg).unwrap(), 2);
    assert_eq!(purge_retired_jobs(&cfg).unwrap(), 1);
    assert_eq!(count_total(&cfg).unwrap(), 1);

    let _ = &d; // silence unused in case of edits
}
