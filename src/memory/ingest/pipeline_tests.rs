use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use chrono::{TimeZone, Utc};
use tempfile::TempDir;

use super::{ingest_chat, ingest_document, ingest_document_versioned, ingest_email_with_raw_refs};
use crate::memory::chunks::{
    count_chunks, get_chunk_content_pointers, get_chunk_lifecycle_status, is_source_ingested,
    SourceKind, CHUNK_STATUS_PENDING_EXTRACTION,
};
use crate::memory::chunks::{get_chunk_raw_refs, RawRef};
use crate::memory::config::MemoryConfig;
use crate::memory::ingest::canonicalize::chat::{ChatBatch, ChatMessage};
use crate::memory::ingest::canonicalize::document::DocumentInput;
use crate::memory::ingest::canonicalize::email::{EmailMessage, EmailThread};
use crate::memory::ingest::types::{NullJobSink, QueueJobSink, TreeJobSink};
use crate::memory::queue::{count_by_status, JobStatus};
use crate::memory::score::ScoringConfig;

/// Records every enqueued extract job id for assertions.
#[derive(Default)]
struct RecordingJobSink {
    ids: Mutex<Vec<String>>,
}

impl TreeJobSink for RecordingJobSink {
    fn enqueue_extract_tx(
        &self,
        _tx: &rusqlite::Transaction<'_>,
        chunk_id: &str,
        _default_max_attempts: u32,
    ) -> anyhow::Result<bool> {
        self.ids.lock().unwrap().push(chunk_id.to_string());
        Ok(true)
    }
}

/// Sink that fails enqueue while `fail` is set, to simulate a post-gate stage
/// error. Records ids on the successful path so a retry can be asserted.
#[derive(Default)]
struct TogglingJobSink {
    fail: AtomicBool,
    ids: Mutex<Vec<String>>,
}

impl TreeJobSink for TogglingJobSink {
    fn enqueue_extract_tx(
        &self,
        _tx: &rusqlite::Transaction<'_>,
        chunk_id: &str,
        _default_max_attempts: u32,
    ) -> anyhow::Result<bool> {
        if self.fail.load(Ordering::SeqCst) {
            anyhow::bail!("simulated enqueue failure");
        }
        self.ids.lock().unwrap().push(chunk_id.to_string());
        Ok(true)
    }
}

fn test_config() -> (TempDir, MemoryConfig) {
    let tmp = TempDir::new().unwrap();
    let cfg = MemoryConfig::new(tmp.path());
    (tmp, cfg)
}

fn substantive_batch() -> ChatBatch {
    ChatBatch {
        platform: "slack".into(),
        channel_label: "#eng".into(),
        messages: vec![
            ChatMessage {
                author: "alice".into(),
                timestamp: Utc.timestamp_millis_opt(1_700_000_000_000).unwrap(),
                text: "We are planning to ship the Phoenix migration on Friday after reviewing the runbook and staging results. alice@example.com"
                    .into(),
                source_ref: Some("slack://m1".into()),
            },
            ChatMessage {
                author: "bob".into(),
                timestamp: Utc.timestamp_millis_opt(1_700_000_010_000).unwrap(),
                text: "Confirmed, I will handle the coordination and launch tracking tonight."
                    .into(),
                source_ref: None,
            },
        ],
    }
}

#[tokio::test]
async fn ingest_chat_writes_chunks_and_enqueues_extract_jobs() {
    let (_tmp, cfg) = test_config();
    let sink = RecordingJobSink::default();
    let scoring = ScoringConfig::default_regex_only();

    let out = ingest_chat(
        &cfg,
        "slack:#eng",
        "alice",
        vec![],
        substantive_batch(),
        &sink,
        &scoring,
    )
    .await
    .unwrap();

    assert!(!out.already_ingested);
    assert!(out.chunks_written >= 1);
    assert_eq!(count_chunks(&cfg).unwrap(), out.chunks_written as u64);
    assert_eq!(out.chunk_ids.len(), out.chunks_written);
    let pointers = get_chunk_content_pointers(&cfg, &out.chunk_ids[0])
        .unwrap()
        .unwrap();
    assert!(!pointers.0.is_empty());
    assert!(!pointers.1.is_empty());

    // Every scheduled chunk got an extract job and is parked at pending.
    assert_eq!(out.extract_jobs_enqueued, out.chunk_ids.len());
    assert_eq!(sink.ids.lock().unwrap().len(), out.extract_jobs_enqueued);
    assert_eq!(
        get_chunk_lifecycle_status(&cfg, &out.chunk_ids[0]).unwrap(),
        Some(CHUNK_STATUS_PENDING_EXTRACTION.to_string())
    );
}

#[tokio::test]
async fn queue_sink_commits_chunk_lifecycle_and_extract_job_together() {
    let (_tmp, cfg) = test_config();
    let out = ingest_chat(
        &cfg,
        "slack:#atomic",
        "alice",
        vec![],
        substantive_batch(),
        &QueueJobSink,
        &ScoringConfig::default_regex_only(),
    )
    .await
    .unwrap();

    assert_eq!(out.extract_jobs_enqueued, out.chunk_ids.len());
    assert_eq!(
        count_by_status(&cfg, JobStatus::Ready).unwrap(),
        out.extract_jobs_enqueued as u64
    );
    for chunk_id in out.chunk_ids {
        assert_eq!(
            get_chunk_lifecycle_status(&cfg, &chunk_id)
                .unwrap()
                .as_deref(),
            Some(CHUNK_STATUS_PENDING_EXTRACTION)
        );
    }
}

#[tokio::test]
async fn ingest_chat_empty_batch_is_noop() {
    let (_tmp, cfg) = test_config();
    let sink = NullJobSink;
    let scoring = ScoringConfig::default_regex_only();
    let batch = ChatBatch {
        platform: "slack".into(),
        channel_label: "#eng".into(),
        messages: vec![],
    };
    let out = ingest_chat(&cfg, "slack:#eng", "alice", vec![], batch, &sink, &scoring)
        .await
        .unwrap();
    assert_eq!(out.chunks_written, 0);
    assert_eq!(out.extract_jobs_enqueued, 0);
    assert_eq!(count_chunks(&cfg).unwrap(), 0);
}

#[tokio::test]
async fn second_document_ingest_with_same_source_id_is_short_circuited() {
    let (_tmp, cfg) = test_config();
    let sink = RecordingJobSink::default();
    let scoring = ScoringConfig::default_regex_only();

    let doc = DocumentInput {
        provider: "notion".into(),
        title: "Launch plan".into(),
        body: "Phoenix ships Friday after staging review. alice@example.com owns this.".into(),
        modified_at: Utc.timestamp_millis_opt(1_700_000_000_000).unwrap(),
        source_ref: Some("notion://page/abc".into()),
    };
    let first = ingest_document(
        &cfg,
        "notion:abc",
        "alice",
        vec![],
        doc.clone(),
        &sink,
        &scoring,
    )
    .await
    .unwrap();
    assert!(!first.already_ingested);
    assert!(first.chunks_written >= 1);
    let staged_files_before = walkdir::WalkDir::new(crate::memory::chunks::content_root(&cfg))
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .count();

    // Even with completely different content under the same source_id the second
    // ingest must write nothing: documents are append-only and source_id is the
    // dedup key.
    let mutated = DocumentInput {
        body: "totally different content that should NOT make it into the tree".into(),
        ..doc
    };
    let second = ingest_document(
        &cfg,
        "notion:abc",
        "alice",
        vec![],
        mutated,
        &sink,
        &scoring,
    )
    .await
    .unwrap();
    assert!(second.already_ingested);
    assert_eq!(second.chunks_written, 0);
    assert!(second.chunk_ids.is_empty());

    assert_eq!(count_chunks(&cfg).unwrap(), first.chunks_written as u64);
    let staged_files_after = walkdir::WalkDir::new(crate::memory::chunks::content_root(&cfg))
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .count();
    assert_eq!(
        staged_files_after, staged_files_before,
        "a losing gate attempt must reclaim its unreferenced staged bodies"
    );
}

#[tokio::test]
async fn failed_document_ingest_rolls_back_gate_so_retry_ingests() {
    let (_tmp, cfg) = test_config();
    let sink = TogglingJobSink::default();
    let scoring = ScoringConfig::default_regex_only();

    let doc = DocumentInput {
        provider: "notion".into(),
        title: "Launch plan".into(),
        body: "Phoenix ships Friday after staging review. alice@example.com owns this.".into(),
        modified_at: Utc.timestamp_millis_opt(1_700_000_000_000).unwrap(),
        source_ref: Some("notion://page/abc".into()),
    };

    // First attempt: enqueue fails inside the database tail. The whole
    // transaction, including the source gate, must roll back.
    sink.fail.store(true, Ordering::SeqCst);
    let err = ingest_document(
        &cfg,
        "notion:abc",
        "alice",
        vec![],
        doc.clone(),
        &sink,
        &scoring,
    )
    .await;
    assert!(err.is_err(), "forced enqueue failure should surface as Err");
    assert!(
        !is_source_ingested(&cfg, SourceKind::Document, "notion:abc").unwrap(),
        "a failed ingest must release the source gate (no zero-chunk claim)",
    );

    // Retry: the gate was released, so the document actually ingests this time.
    sink.fail.store(false, Ordering::SeqCst);
    let retry = ingest_document(&cfg, "notion:abc", "alice", vec![], doc, &sink, &scoring)
        .await
        .unwrap();
    assert!(
        !retry.already_ingested,
        "retry after a failed ingest must not report already-ingested",
    );
    assert!(retry.chunks_written >= 1);
    assert_eq!(retry.extract_jobs_enqueued, retry.chunk_ids.len());
    assert!(is_source_ingested(&cfg, SourceKind::Document, "notion:abc").unwrap());
}

#[tokio::test]
async fn versioned_document_admits_second_revision() {
    let (_tmp, cfg) = test_config();
    let sink = RecordingJobSink::default();
    let scoring = ScoringConfig::default_regex_only();

    let doc_v1 = DocumentInput {
        provider: "notion".into(),
        title: "Roadmap".into(),
        body: "Phoenix ships Friday. alice@example.com owns the rollout.".into(),
        modified_at: Utc.timestamp_millis_opt(1_700_000_000_000).unwrap(),
        source_ref: Some("notion://page/v".into()),
    };
    let v1 = ingest_document_versioned(
        &cfg,
        "notion:v",
        "alice",
        vec![],
        doc_v1,
        None,
        Some(1),
        &sink,
        &scoring,
    )
    .await
    .unwrap();
    assert!(!v1.already_ingested);
    assert!(v1.chunks_written >= 1);

    let doc_v2 = DocumentInput {
        provider: "notion".into(),
        title: "Roadmap".into(),
        body: "Phoenix now ships next Monday. bob@example.com owns the rollout.".into(),
        modified_at: Utc.timestamp_millis_opt(1_700_000_100_000).unwrap(),
        source_ref: Some("notion://page/v".into()),
    };
    let v2 = ingest_document_versioned(
        &cfg,
        "notion:v",
        "alice",
        vec![],
        doc_v2,
        None,
        Some(2),
        &sink,
        &scoring,
    )
    .await
    .unwrap();
    assert!(!v2.already_ingested, "a new version must be admitted");
    assert!(v2.chunks_written >= 1);
}

#[tokio::test]
async fn ingest_email_with_raw_refs_attaches_archive_refs_to_every_chunk() {
    let (_tmp, cfg) = test_config();
    let sink = RecordingJobSink::default();
    let scoring = ScoringConfig::default_regex_only();
    let thread = EmailThread {
        provider: "gmail".into(),
        thread_subject: "Launch review".into(),
        messages: vec![
            EmailMessage {
                from: "Alice Smith <alice@example.com>".into(),
                to: vec!["Bob Jones <bob@example.com>".into()],
                cc: vec![],
                subject: "Launch review".into(),
                sent_at: Utc.timestamp_millis_opt(1_700_000_000_000).unwrap(),
                body: "Alice sent the launch checklist to Bob. Kitchen is north of Garden.".into(),
                source_ref: Some("gmail://msg-1".into()),
                list_unsubscribe: None,
            },
            EmailMessage {
                from: "Bob Jones <bob@example.com>".into(),
                to: vec!["Alice Smith <alice@example.com>".into()],
                cc: vec![],
                subject: "Re: Launch review".into(),
                sent_at: Utc.timestamp_millis_opt(1_700_000_060_000).unwrap(),
                body: "Bob confirmed the checklist and added staging notes.".into(),
                source_ref: Some("gmail://msg-2".into()),
                list_unsubscribe: None,
            },
        ],
    };
    let raw_refs = vec![
        RawRef {
            path: "raw/gmail/thread-1/message-1.md".into(),
            start: 0,
            end: Some(128),
        },
        RawRef {
            path: "raw/gmail/thread-1/message-2.md".into(),
            start: 128,
            end: None,
        },
    ];

    let out = ingest_email_with_raw_refs(
        &cfg,
        "gmail:thread-1",
        "alice",
        vec!["mock-email".into()],
        thread,
        raw_refs.clone(),
        &sink,
        &scoring,
    )
    .await
    .unwrap();

    assert!(!out.already_ingested);
    assert!(out.chunks_written >= 1);
    assert_eq!(out.extract_jobs_enqueued, out.chunk_ids.len());
    for chunk_id in &out.chunk_ids {
        let stored = get_chunk_raw_refs(&cfg, chunk_id).unwrap().unwrap();
        assert_eq!(stored.len(), raw_refs.len());
        assert_eq!(stored[0].path, raw_refs[0].path);
        assert_eq!(stored[1].start, raw_refs[1].start);
    }
}
