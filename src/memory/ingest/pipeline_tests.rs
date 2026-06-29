use std::sync::Mutex;

use chrono::{TimeZone, Utc};
use tempfile::TempDir;

use super::{ingest_chat, ingest_document, ingest_document_versioned};
use crate::memory::chunks::{
    count_chunks, get_chunk_lifecycle_status, CHUNK_STATUS_PENDING_EXTRACTION,
};
use crate::memory::config::MemoryConfig;
use crate::memory::ingest::canonicalize::chat::{ChatBatch, ChatMessage};
use crate::memory::ingest::canonicalize::document::DocumentInput;
use crate::memory::ingest::types::{NullJobSink, TreeJobSink};
use crate::memory::score::ScoringConfig;

/// Records every enqueued extract job id for assertions.
#[derive(Default)]
struct RecordingJobSink {
    ids: Mutex<Vec<String>>,
}

impl TreeJobSink for RecordingJobSink {
    fn enqueue_extract(&self, chunk_id: &str) -> anyhow::Result<()> {
        self.ids.lock().unwrap().push(chunk_id.to_string());
        Ok(())
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

    // Every scheduled chunk got an extract job and is parked at pending.
    assert_eq!(out.extract_jobs_enqueued, out.chunk_ids.len());
    assert_eq!(sink.ids.lock().unwrap().len(), out.extract_jobs_enqueued);
    assert_eq!(
        get_chunk_lifecycle_status(&cfg, &out.chunk_ids[0]).unwrap(),
        Some(CHUNK_STATUS_PENDING_EXTRACTION.to_string())
    );
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
