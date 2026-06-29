use super::*;

#[test]
fn job_kind_roundtrip() {
    for k in [
        JobKind::ExtractChunk,
        JobKind::AppendBuffer,
        JobKind::Seal,
        JobKind::FlushStale,
        JobKind::ReembedBackfill,
        JobKind::SealDocument,
    ] {
        assert_eq!(JobKind::parse(k.as_str()).unwrap(), k);
    }
    // Retired kinds parse to an error (global/topic trees removed).
    assert!(JobKind::parse("topic_route").is_err());
    assert!(JobKind::parse("digest_daily").is_err());
    // Unknown kinds also error.
    assert!(JobKind::parse("not_a_kind").is_err());
}

#[test]
fn seal_document_dedupe_key_is_per_version() {
    let v1 = SealDocumentPayload {
        tree_scope: "notion:conn1".into(),
        doc_id: "notion:conn1:pageA".into(),
        version_ms: Some(1717000000000),
        chunk_ids: vec!["c0".into()],
    };
    let v2 = SealDocumentPayload {
        version_ms: Some(1717500000000),
        ..v1.clone()
    };
    // Distinct versions of the same doc get distinct keys.
    assert_ne!(v1.dedupe_key(), v2.dedupe_key());
    assert_eq!(v1.dedupe_key(), "seal_doc:notion:conn1:pageA@1717000000000");
    // Unversioned falls back to the bare doc id.
    let unversioned = SealDocumentPayload {
        version_ms: None,
        ..v1.clone()
    };
    assert_eq!(unversioned.dedupe_key(), "seal_doc:notion:conn1:pageA");
}

#[test]
fn seal_document_roundtrips_through_newjob() {
    let p = SealDocumentPayload {
        tree_scope: "notion:conn1".into(),
        doc_id: "notion:conn1:pageA".into(),
        version_ms: Some(42),
        chunk_ids: vec!["c0".into(), "c1".into()],
    };
    let job = NewJob::seal_document(&p).unwrap();
    assert_eq!(job.kind, JobKind::SealDocument);
    let back: SealDocumentPayload = serde_json::from_str(&job.payload_json).unwrap();
    assert_eq!(back.chunk_ids, vec!["c0".to_string(), "c1".to_string()]);
    assert_eq!(back.version_ms, Some(42));
}

#[test]
fn job_status_terminality() {
    assert!(!JobStatus::Ready.is_terminal());
    assert!(!JobStatus::Running.is_terminal());
    assert!(JobStatus::Done.is_terminal());
    assert!(JobStatus::Failed.is_terminal());
    assert!(JobStatus::Cancelled.is_terminal());
}

#[test]
fn job_status_roundtrip() {
    for s in [
        JobStatus::Ready,
        JobStatus::Running,
        JobStatus::Done,
        JobStatus::Failed,
        JobStatus::Cancelled,
    ] {
        assert_eq!(JobStatus::parse(s.as_str()).unwrap(), s);
    }
    assert!(JobStatus::parse("bogus").is_err());
}

#[test]
fn dedupe_keys_distinguish_targets() {
    let p_src = AppendBufferPayload {
        node: NodeRef::Leaf {
            chunk_id: "c1".into(),
        },
        target: AppendTarget::Source {
            source_id: "slack:#eng".into(),
        },
    };
    let p_topic = AppendBufferPayload {
        node: NodeRef::Leaf {
            chunk_id: "c1".into(),
        },
        target: AppendTarget::Topic {
            tree_id: "topic:abc".into(),
        },
    };
    assert_ne!(p_src.dedupe_key(), p_topic.dedupe_key());
}

#[test]
fn dedupe_keys_distinguish_node_kinds() {
    let p_leaf = AppendBufferPayload {
        node: NodeRef::Leaf {
            chunk_id: "x".into(),
        },
        target: AppendTarget::Topic {
            tree_id: "t".into(),
        },
    };
    let p_summary = AppendBufferPayload {
        node: NodeRef::Summary {
            summary_id: "x".into(),
        },
        target: AppendTarget::Topic {
            tree_id: "t".into(),
        },
    };
    assert_ne!(p_leaf.dedupe_key(), p_summary.dedupe_key());
}

#[test]
fn flush_stale_dedupe_key_is_pure_and_per_3h_block() {
    let p = FlushStalePayload::default();
    assert_eq!(p.dedupe_key("2026-05-19", 2), p.dedupe_key("2026-05-19", 2));
    assert_ne!(p.dedupe_key("2026-05-19", 2), p.dedupe_key("2026-05-19", 3));
    assert_ne!(p.dedupe_key("2026-05-19", 2), p.dedupe_key("2026-05-20", 2));
    assert_eq!(p.dedupe_key("2026-05-19", 0), "flush_stale:2026-05-19-h0");
    assert_eq!(p.dedupe_key("2026-05-19", 7), "flush_stale:2026-05-19-h7");
}

#[test]
fn llm_bound_kinds() {
    assert!(JobKind::ExtractChunk.is_llm_bound());
    assert!(JobKind::Seal.is_llm_bound());
    assert!(JobKind::SealDocument.is_llm_bound());
    assert!(JobKind::ReembedBackfill.is_llm_bound());
    assert!(!JobKind::AppendBuffer.is_llm_bound());
    assert!(!JobKind::FlushStale.is_llm_bound());
}

#[test]
fn node_ref_serializes_with_kind_tag() {
    let leaf = NodeRef::Leaf {
        chunk_id: "x".into(),
    };
    let s = serde_json::to_string(&leaf).unwrap();
    assert!(s.contains("\"kind\":\"leaf\""));
    let back: NodeRef = serde_json::from_str(&s).unwrap();
    assert_eq!(back, leaf);
}

#[test]
fn append_target_serializes_with_kind_tag() {
    let p = AppendTarget::Source {
        source_id: "x".into(),
    };
    let s = serde_json::to_string(&p).unwrap();
    assert!(s.contains("\"kind\":\"source\""));
    assert!(s.contains("\"source_id\":\"x\""));
    let back: AppendTarget = serde_json::from_str(&s).unwrap();
    match back {
        AppendTarget::Source { source_id } => assert_eq!(source_id, "x"),
        _ => panic!("wrong variant"),
    }
}

#[test]
fn new_job_extract_chunk_builder_sets_kind_payload_and_dedupe_key() {
    let payload = ExtractChunkPayload {
        chunk_id: "chunk-123".into(),
    };
    let job = NewJob::extract_chunk(&payload).unwrap();
    assert_eq!(job.kind, JobKind::ExtractChunk);
    assert_eq!(job.dedupe_key.as_deref(), Some("extract:chunk-123"));
    assert_eq!(job.available_at_ms, None);
    assert_eq!(job.max_attempts, None);
    let roundtrip: ExtractChunkPayload = serde_json::from_str(&job.payload_json).unwrap();
    assert_eq!(roundtrip.chunk_id, "chunk-123");
}

#[test]
fn new_job_append_buffer_builder_uses_payload_dedupe_key() {
    let payload = AppendBufferPayload {
        node: NodeRef::Summary {
            summary_id: "summary-9".into(),
        },
        target: AppendTarget::Topic {
            tree_id: "topic:ops".into(),
        },
    };
    let job = NewJob::append_buffer(&payload).unwrap();
    assert_eq!(job.kind, JobKind::AppendBuffer);
    assert_eq!(
        job.dedupe_key.as_deref(),
        Some("append:topic:topic:ops:summary:summary-9")
    );
    let roundtrip: AppendBufferPayload = serde_json::from_str(&job.payload_json).unwrap();
    assert_eq!(roundtrip.dedupe_key(), payload.dedupe_key());
}

#[test]
fn new_job_flush_stale_builder_uses_supplied_time_bucket() {
    let payload = FlushStalePayload {
        max_age_secs: Some(600),
    };
    let job = NewJob::flush_stale(&payload, "2026-05-24", 4).unwrap();
    assert_eq!(job.kind, JobKind::FlushStale);
    assert_eq!(job.dedupe_key.as_deref(), Some("flush_stale:2026-05-24-h4"));
    let roundtrip: FlushStalePayload = serde_json::from_str(&job.payload_json).unwrap();
    assert_eq!(roundtrip.max_age_secs, Some(600));
}

#[test]
fn new_job_reembed_backfill_builder_is_one_chain_per_signature() {
    let payload = ReembedBackfillPayload {
        signature: "embed-v2".into(),
    };
    let job = NewJob::reembed_backfill(&payload).unwrap();
    assert_eq!(job.kind, JobKind::ReembedBackfill);
    assert_eq!(job.dedupe_key.as_deref(), Some("reembed_backfill:embed-v2"));
    assert_eq!(job.max_attempts, Some(3));
    let roundtrip: ReembedBackfillPayload = serde_json::from_str(&job.payload_json).unwrap();
    assert_eq!(roundtrip.signature, "embed-v2");
}

#[test]
fn job_failure_classification() {
    let unrec = JobFailure::budget_exhausted();
    assert!(unrec.is_unrecoverable());
    assert_eq!(unrec.code, "budget_exhausted");
    assert_eq!(unrec.class, "unrecoverable");

    let trans = JobFailure::transient("upstream_503");
    assert!(!trans.is_unrecoverable());
    assert_eq!(trans.class, "transient");

    // It must be usable as an anyhow source so the worker can downcast it.
    let err = anyhow::Error::new(JobFailure::budget_exhausted()).context("embed failed");
    assert!(err.downcast_ref::<JobFailure>().unwrap().is_unrecoverable());
}
