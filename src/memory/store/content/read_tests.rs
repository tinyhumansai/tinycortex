use super::*;
use crate::memory::chunks::{Chunk, Metadata, SourceKind};
use crate::memory::store::content::atomic::{sha256_hex, write_if_new};
use crate::memory::store::content::compose::{
    compose_chunk_file, compose_summary_md, SummaryComposeInput,
};
use crate::memory::store::content::paths::SummaryTreeKind;
use chrono::TimeZone;
use tempfile::TempDir;

fn sample_chunk() -> Chunk {
    let ts = chrono::Utc.timestamp_millis_opt(1_700_000_000_000).unwrap();
    Chunk {
        id: "read_test".into(),
        content: "## ts — alice\nhello from read test".into(),
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
        token_count: 8,
        seq_in_source: 0,
        created_at: ts,
        partial_message: false,
    }
}

#[test]
fn read_returns_body_and_correct_sha256() {
    let dir = TempDir::new().unwrap();
    let chunk = sample_chunk();
    let (full_bytes, body_bytes) = compose_chunk_file(&chunk);
    let path = dir.path().join("0.md");
    write_if_new(&path, &full_bytes).unwrap();

    let result = read_chunk_file(&path).unwrap();
    assert_eq!(result.body, std::str::from_utf8(&body_bytes).unwrap());
    assert_eq!(result.sha256, sha256_hex(&body_bytes));
}

#[test]
fn verify_passes_for_correct_hash() {
    let dir = TempDir::new().unwrap();
    let chunk = sample_chunk();
    let (full_bytes, body_bytes) = compose_chunk_file(&chunk);
    let path = dir.path().join("0.md");
    write_if_new(&path, &full_bytes).unwrap();

    let expected = sha256_hex(&body_bytes);
    assert!(verify_chunk_file(&path, &expected).unwrap());
}

#[test]
fn verify_fails_for_wrong_hash() {
    let dir = TempDir::new().unwrap();
    let chunk = sample_chunk();
    let (full_bytes, _) = compose_chunk_file(&chunk);
    let path = dir.path().join("0.md");
    write_if_new(&path, &full_bytes).unwrap();

    assert!(!verify_chunk_file(&path, "deadbeef").unwrap());
}

#[test]
fn read_missing_file_returns_error() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("nonexistent.md");
    assert!(read_chunk_file(&path).is_err());
}

fn write_summary_file(dir: &TempDir, body: &str) -> (std::path::PathBuf, String) {
    let ts = chrono::Utc.timestamp_millis_opt(1_700_000_000_000).unwrap();
    let input = SummaryComposeInput {
        summary_id: "sum:L1:readtest",
        tree_kind: SummaryTreeKind::Source,
        tree_id: "t1",
        tree_scope: "gmail:alice@x.com",
        level: 1,
        child_ids: &["c1".to_string()],
        child_basenames: None,
        child_count: 1,
        time_range_start: ts,
        time_range_end: ts,
        sealed_at: ts,
        body,
    };
    let composed = compose_summary_md(&input);
    let path = dir.path().join("sum.md");
    let sha = sha256_hex(composed.body.as_bytes());
    write_if_new(&path, composed.full.as_bytes()).unwrap();
    (path, sha)
}

#[test]
fn read_summary_file_returns_body_and_sha() {
    let dir = TempDir::new().unwrap();
    let body = "summary body content\n";
    let (path, expected_sha) = write_summary_file(&dir, body);
    let result = read_summary_file(&path).unwrap();
    assert_eq!(result.body, body);
    assert_eq!(result.sha256, expected_sha);
}

#[test]
fn verify_summary_file_ok_for_correct_hash() {
    let dir = TempDir::new().unwrap();
    let (path, sha) = write_summary_file(&dir, "body text\n");
    assert_eq!(verify_summary_file(&path, &sha).unwrap(), VerifyResult::Ok);
}

#[test]
fn verify_summary_file_mismatch_for_wrong_hash() {
    let dir = TempDir::new().unwrap();
    let (path, _) = write_summary_file(&dir, "body text\n");
    let r = verify_summary_file(&path, "deadbeef").unwrap();
    assert!(matches!(r, VerifyResult::Mismatch { .. }));
}

#[test]
fn verify_summary_file_missing_for_absent_file() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("does_not_exist.md");
    assert_eq!(
        verify_summary_file(&path, "abc").unwrap(),
        VerifyResult::Missing
    );
}

#[test]
fn read_chunk_file_rejects_invalid_utf8() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("bad.md");
    std::fs::write(&path, [0xff, 0xfe, 0xfd]).unwrap();
    let err = read_chunk_file(&path).unwrap_err();
    assert!(err.to_string().contains("invalid UTF-8"));
}

#[test]
fn read_chunk_file_rejects_missing_front_matter() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("plain.md");
    std::fs::write(&path, "no front matter here").unwrap();
    let err = read_chunk_file(&path).unwrap_err();
    assert!(err.to_string().contains("no front-matter"));
}

#[test]
fn verify_summary_file_mismatch_returns_actual_sha() {
    let dir = TempDir::new().unwrap();
    let (path, expected_sha) = write_summary_file(&dir, "body text\n");
    let actual = match verify_summary_file(&path, "deadbeef").unwrap() {
        VerifyResult::Mismatch { actual } => actual,
        other => panic!("expected mismatch, got {other:?}"),
    };
    assert_eq!(actual, expected_sha);
}

#[test]
fn resolve_within_content_root_rejects_traversal_and_absolute() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    assert!(resolve_within_content_root(root, "../escape.md").is_err());
    assert!(resolve_within_content_root(root, "a/../../escape.md").is_err());
    assert!(resolve_within_content_root(root, "/etc/passwd").is_err());

    let ok = resolve_within_content_root(root, "sub/dir/file.md").unwrap();
    assert_eq!(ok, root.join("sub").join("dir").join("file.md"));
}

#[test]
fn high_level_chunk_reader_uses_custom_root_and_repairs_checksum() {
    let dir = TempDir::new().unwrap();
    let custom_root = dir.path().join("custom-content");
    let mut config = crate::memory::MemoryConfig::new(dir.path());
    config.content_root = Some(custom_root.clone());
    let chunk = sample_chunk();
    let staged =
        crate::memory::store::content::stage_chunks(&custom_root, &[chunk.clone()]).unwrap();
    crate::memory::chunks::with_connection(&config, |conn| {
        let tx = conn.unchecked_transaction()?;
        crate::memory::chunks::upsert_staged_chunks_tx(&tx, &staged)?;
        tx.commit()?;
        Ok(())
    })
    .unwrap();
    crate::memory::chunks::update_chunk_content_sha256(&config, &chunk.id, "stale").unwrap();

    assert_eq!(read_chunk_body(&config, &chunk.id).unwrap(), chunk.content);
    let (_, repaired) = crate::memory::chunks::get_chunk_content_pointers(&config, &chunk.id)
        .unwrap()
        .unwrap();
    assert_eq!(repaired, staged[0].content_sha256);
}

#[test]
fn high_level_chunk_reader_joins_clamped_raw_references() {
    let dir = TempDir::new().unwrap();
    let mut config = crate::memory::MemoryConfig::new(dir.path());
    let root = dir.path().join("raw-root");
    config.content_root = Some(root.clone());
    std::fs::create_dir_all(root.join("raw/source")).unwrap();
    std::fs::write(root.join("raw/source/item.md"), "alpha beta").unwrap();
    let chunk = sample_chunk();
    crate::memory::chunks::upsert_chunks(&config, &[chunk.clone()]).unwrap();
    crate::memory::chunks::set_chunk_raw_refs(
        &config,
        &chunk.id,
        &[
            crate::memory::chunks::RawRef {
                path: "raw/source/item.md".into(),
                start: 0,
                end: Some(5),
            },
            crate::memory::chunks::RawRef {
                path: "raw/source/item.md".into(),
                start: 6,
                end: Some(100),
            },
        ],
    )
    .unwrap();

    assert_eq!(
        read_chunk_body(&config, &chunk.id).unwrap(),
        "alpha\n\nbeta"
    );
}
