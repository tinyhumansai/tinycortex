use super::*;
use crate::memory::chunks::{Chunk, Metadata, SourceKind};
use chrono::TimeZone;
use tempfile::TempDir;

fn sample_chunk(seq: u32) -> Chunk {
    let ts = chrono::Utc
        .timestamp_millis_opt(1_700_000_000_000 + seq as i64)
        .unwrap();
    Chunk {
        id: format!("chunk_{seq}"),
        content: format!("## ts — alice\nMessage {seq}"),
        metadata: Metadata {
            source_kind: SourceKind::Chat,
            source_id: "slack:#eng".into(),
            owner: "alice".into(),
            timestamp: ts,
            time_range: (ts, ts),
            tags: vec![],
            source_ref: None,
            path_scope: None,
        },
        token_count: 5,
        seq_in_source: seq,
        created_at: ts,
        partial_message: false,
    }
}

#[test]
fn stage_chunks_writes_files_and_returns_staged() {
    let dir = TempDir::new().unwrap();
    let chunks = vec![sample_chunk(0), sample_chunk(1)];
    let staged = stage_chunks(dir.path(), &chunks).unwrap();

    assert_eq!(staged.len(), 2);
    for s in &staged {
        let abs = paths::chunk_abs_path(
            dir.path(),
            s.chunk.metadata.source_kind.as_str(),
            &s.chunk.metadata.source_id,
            &s.chunk.id,
        );
        assert!(abs.exists(), "file must exist: {}", abs.display());
        assert!(!s.content_path.is_empty());
        assert_eq!(s.content_sha256.len(), 64);
        assert!(!s.content_path.starts_with('/'));
        assert!(s.content_path.contains('/'));
    }
}

#[test]
fn stage_chunks_is_idempotent() {
    let dir = TempDir::new().unwrap();
    let chunks = vec![sample_chunk(0)];
    let first = stage_chunks(dir.path(), &chunks).unwrap();
    let second = stage_chunks(dir.path(), &chunks).unwrap();
    assert_eq!(first[0].content_sha256, second[0].content_sha256);
    assert_eq!(first[0].content_path, second[0].content_path);
}

#[test]
fn stage_chunks_email_skips_disk_write() {
    let dir = TempDir::new().unwrap();
    let ts = chrono::Utc.timestamp_millis_opt(1_700_000_000_000).unwrap();
    let email = Chunk {
        id: "email_chunk".into(),
        content: "body".into(),
        metadata: Metadata {
            source_kind: SourceKind::Email,
            source_id: "gmail:alice@x.com".into(),
            owner: "alice".into(),
            timestamp: ts,
            time_range: (ts, ts),
            tags: vec![],
            source_ref: None,
            path_scope: None,
        },
        token_count: 1,
        seq_in_source: 0,
        created_at: ts,
        partial_message: false,
    };
    let staged = stage_chunks(dir.path(), &[email]).unwrap();
    assert_eq!(staged.len(), 1);
    assert!(staged[0].content_path.is_empty());
    assert!(staged[0].content_sha256.is_empty());
}
