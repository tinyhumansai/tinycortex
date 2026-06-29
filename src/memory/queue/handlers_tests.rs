use super::*;
use crate::memory::chunks::with_connection;
use crate::memory::config::MemoryConfig;
use crate::memory::queue::test_support::RecordingDelegates;
use crate::memory::queue::types::{JobStatus, ReembedBackfillPayload};
use rusqlite::params;
use std::collections::VecDeque;
use tempfile::TempDir;

fn test_config() -> (TempDir, MemoryConfig) {
    let tmp = TempDir::new().unwrap();
    let cfg = MemoryConfig::new(tmp.path());
    (tmp, cfg)
}

fn mk_running_job(kind: JobKind, payload_json: String) -> Job {
    let now_ms = chrono::Utc::now().timestamp_millis();
    Job {
        id: "test-job-id".into(),
        kind,
        payload_json,
        dedupe_key: None,
        status: JobStatus::Running,
        attempts: 1,
        max_attempts: 5,
        available_at_ms: now_ms,
        locked_until_ms: Some(now_ms + 60_000),
        last_error: None,
        created_at_ms: now_ms,
        started_at_ms: Some(now_ms),
        completed_at_ms: None,
        failure_reason: None,
        failure_class: None,
    }
}

fn count_jobs_of_kind(cfg: &MemoryConfig, kind: &str) -> u64 {
    with_connection(cfg, |conn| {
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM mem_tree_jobs WHERE kind = ?1",
            params![kind],
            |r| r.get(0),
        )?;
        Ok(n.max(0) as u64)
    })
    .unwrap()
}

#[tokio::test]
async fn extract_admits_and_enqueues_append_buffer() {
    let (_tmp, cfg) = test_config();
    let d = RecordingDelegates::admitting();
    let job = mk_running_job(
        JobKind::ExtractChunk,
        serde_json::to_string(&ExtractChunkPayload {
            chunk_id: "c1".into(),
        })
        .unwrap(),
    );
    let out = handle_job(&cfg, &job, &d).await.unwrap();
    assert_eq!(out, JobOutcome::Done);
    assert_eq!(count_jobs_of_kind(&cfg, "append_buffer"), 1);
    // Covered space → no reembed chain armed.
    assert_eq!(count_jobs_of_kind(&cfg, "reembed_backfill"), 0);
}

#[tokio::test]
async fn extract_missing_chunk_is_noop_done() {
    let (_tmp, cfg) = test_config();
    let mut d = RecordingDelegates::admitting();
    d.extract = None; // simulate missing chunk row
    let job = mk_running_job(
        JobKind::ExtractChunk,
        serde_json::to_string(&ExtractChunkPayload {
            chunk_id: "gone".into(),
        })
        .unwrap(),
    );
    assert_eq!(handle_job(&cfg, &job, &d).await.unwrap(), JobOutcome::Done);
    assert_eq!(count_jobs_of_kind(&cfg, "append_buffer"), 0);
}

#[tokio::test]
async fn extract_dropped_chunk_enqueues_nothing() {
    let (_tmp, cfg) = test_config();
    let mut d = RecordingDelegates::admitting();
    d.extract = Some(ExtractDecision {
        kept: false,
        uses_document_subtree: false,
        tree_scope: "slack:#eng".into(),
    });
    let job = mk_running_job(
        JobKind::ExtractChunk,
        serde_json::to_string(&ExtractChunkPayload {
            chunk_id: "c1".into(),
        })
        .unwrap(),
    );
    handle_job(&cfg, &job, &d).await.unwrap();
    assert_eq!(count_jobs_of_kind(&cfg, "append_buffer"), 0);
}

#[tokio::test]
async fn extract_admits_doc_subtree_skips_flat_append() {
    let (_tmp, cfg) = test_config();
    let mut d = RecordingDelegates::admitting();
    d.extract = Some(ExtractDecision {
        kept: true,
        uses_document_subtree: true,
        tree_scope: "notion:conn1".into(),
    });
    let job = mk_running_job(
        JobKind::ExtractChunk,
        serde_json::to_string(&ExtractChunkPayload {
            chunk_id: "c1".into(),
        })
        .unwrap(),
    );
    handle_job(&cfg, &job, &d).await.unwrap();
    assert_eq!(
        count_jobs_of_kind(&cfg, "append_buffer"),
        0,
        "document-subtree sources don't use the flat L0 buffer"
    );
}

#[tokio::test]
async fn extract_arms_reembed_when_uncovered() {
    let (_tmp, cfg) = test_config();
    let mut d = RecordingDelegates::admitting();
    d.uncovered = true;
    let job = mk_running_job(
        JobKind::ExtractChunk,
        serde_json::to_string(&ExtractChunkPayload {
            chunk_id: "c1".into(),
        })
        .unwrap(),
    );
    handle_job(&cfg, &job, &d).await.unwrap();
    assert_eq!(count_jobs_of_kind(&cfg, "reembed_backfill"), 1);
}

#[tokio::test]
async fn append_buffer_enqueues_seal_when_gate_met() {
    let (_tmp, cfg) = test_config();
    let d = RecordingDelegates::admitting();
    let payload = AppendBufferPayload {
        node: NodeRef::Leaf {
            chunk_id: "c1".into(),
        },
        target: AppendTarget::Source {
            source_id: "slack:#eng".into(),
        },
    };
    let job = mk_running_job(
        JobKind::AppendBuffer,
        serde_json::to_string(&payload).unwrap(),
    );
    handle_job(&cfg, &job, &d).await.unwrap();
    assert_eq!(count_jobs_of_kind(&cfg, "seal"), 1);
}

#[tokio::test]
async fn append_buffer_no_seal_when_gate_unmet() {
    let (_tmp, cfg) = test_config();
    let mut d = RecordingDelegates::admitting();
    d.append = Some(AppendDecision {
        tree_id: "tree:slack".into(),
        should_seal: false,
    });
    let payload = AppendBufferPayload {
        node: NodeRef::Leaf {
            chunk_id: "c1".into(),
        },
        target: AppendTarget::Source {
            source_id: "slack:#eng".into(),
        },
    };
    let job = mk_running_job(
        JobKind::AppendBuffer,
        serde_json::to_string(&payload).unwrap(),
    );
    handle_job(&cfg, &job, &d).await.unwrap();
    assert_eq!(count_jobs_of_kind(&cfg, "seal"), 0);
}

#[tokio::test]
async fn seal_enqueues_parent_when_cascading() {
    let (_tmp, cfg) = test_config();
    let d = RecordingDelegates::admitting();
    *d.seal_parent.lock() = Some(SealPayload {
        tree_id: "tree:slack".into(),
        level: 1,
        force_now_ms: None,
    });
    let payload = SealPayload {
        tree_id: "tree:slack".into(),
        level: 0,
        force_now_ms: None,
    };
    let job = mk_running_job(JobKind::Seal, serde_json::to_string(&payload).unwrap());
    handle_job(&cfg, &job, &d).await.unwrap();
    // The level-1 parent seal is now queued (distinct dedupe key from level 0).
    assert_eq!(count_jobs_of_kind(&cfg, "seal"), 1);
}

#[tokio::test]
async fn flush_stale_enqueues_a_seal_per_stale_buffer() {
    let (_tmp, cfg) = test_config();
    let mut d = RecordingDelegates::admitting();
    d.stale = vec![
        StaleBuffer {
            tree_id: "tree:a".into(),
            level: 0,
        },
        StaleBuffer {
            tree_id: "tree:b".into(),
            level: 0,
        },
    ];
    let job = mk_running_job(
        JobKind::FlushStale,
        serde_json::to_string(&FlushStalePayload::default()).unwrap(),
    );
    handle_job(&cfg, &job, &d).await.unwrap();
    assert_eq!(count_jobs_of_kind(&cfg, "seal"), 2);
}

#[tokio::test]
async fn seal_document_empty_is_noop() {
    let (_tmp, cfg) = test_config();
    let d = RecordingDelegates::admitting();
    let payload = SealDocumentPayload {
        tree_scope: "notion:conn1".into(),
        doc_id: "notion:conn1:p".into(),
        version_ms: Some(1),
        chunk_ids: vec![],
    };
    let job = mk_running_job(
        JobKind::SealDocument,
        serde_json::to_string(&payload).unwrap(),
    );
    assert_eq!(handle_job(&cfg, &job, &d).await.unwrap(), JobOutcome::Done);
    assert_eq!(
        d.counts
            .seal_document
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );
}

#[tokio::test]
async fn seal_document_nonempty_calls_delegate() {
    let (_tmp, cfg) = test_config();
    let d = RecordingDelegates::admitting();
    let payload = SealDocumentPayload {
        tree_scope: "notion:conn1".into(),
        doc_id: "notion:conn1:p".into(),
        version_ms: Some(1),
        chunk_ids: vec!["c0".into()],
    };
    let job = mk_running_job(
        JobKind::SealDocument,
        serde_json::to_string(&payload).unwrap(),
    );
    handle_job(&cfg, &job, &d).await.unwrap();
    assert_eq!(
        d.counts
            .seal_document
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
}

#[tokio::test]
async fn reembed_defers_when_more_pending_then_done() {
    let (_tmp, cfg) = test_config();
    let d = RecordingDelegates::admitting();
    *d.reembed.lock() = VecDeque::from([
        ReembedProgress::Wrote { more_pending: true },
        ReembedProgress::Covered,
    ]);
    let job = mk_running_job(
        JobKind::ReembedBackfill,
        serde_json::to_string(&ReembedBackfillPayload {
            signature: "sig".into(),
        })
        .unwrap(),
    );
    let out = handle_job(&cfg, &job, &d).await.unwrap();
    assert!(
        matches!(out, JobOutcome::Defer { .. }),
        "more pending → Defer"
    );

    let out2 = handle_job(&cfg, &job, &d).await.unwrap();
    assert_eq!(out2, JobOutcome::Done, "covered → Done");
}

#[tokio::test]
async fn reembed_no_provider_completes_without_defer() {
    let (_tmp, cfg) = test_config();
    let d = RecordingDelegates::admitting();
    *d.reembed.lock() = VecDeque::from([ReembedProgress::NoProvider]);
    let job = mk_running_job(
        JobKind::ReembedBackfill,
        serde_json::to_string(&ReembedBackfillPayload {
            signature: "sig".into(),
        })
        .unwrap(),
    );
    assert_eq!(handle_job(&cfg, &job, &d).await.unwrap(), JobOutcome::Done);
}
