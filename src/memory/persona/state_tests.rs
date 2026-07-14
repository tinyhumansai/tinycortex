//! Tests for persona incremental-run state.

use super::*;
use tempfile::TempDir;

#[tokio::test]
async fn file_cursor_detects_change() {
    let dir = TempDir::new().unwrap();
    let f = dir.path().join("a.jsonl");
    std::fs::write(&f, "one").unwrap();
    let store = FileStateStore::open_in_workspace(dir.path()).unwrap();
    let key = file_key("claude_code", &f);

    // Unrecorded → not unchanged.
    assert!(!file_unchanged(&store, &key, &f).await.unwrap());
    record_file(&store, &key, &f).await.unwrap();
    assert!(file_unchanged(&store, &key, &f).await.unwrap());

    // Append changes len → cursor differs.
    std::fs::write(&f, "one-two-three").unwrap();
    assert!(!file_unchanged(&store, &key, &f).await.unwrap());
}

#[tokio::test]
async fn watermark_roundtrips() {
    let dir = TempDir::new().unwrap();
    let store = FileStateStore::open_in_workspace(dir.path()).unwrap();
    assert!(!watermark_unchanged(&store, "git_history:/r", "sha1").await.unwrap());
    record_watermark(&store, "git_history:/r", "sha1").await.unwrap();
    assert!(watermark_unchanged(&store, "git_history:/r", "sha1").await.unwrap());
    assert!(!watermark_unchanged(&store, "git_history:/r", "sha2").await.unwrap());
}

#[tokio::test]
async fn state_persists_across_reopen() {
    let dir = TempDir::new().unwrap();
    {
        let store = FileStateStore::open_in_workspace(dir.path()).unwrap();
        record_watermark(&store, "k", "v").await.unwrap();
    }
    // Reopen from disk.
    let store = FileStateStore::open_in_workspace(dir.path()).unwrap();
    assert!(watermark_unchanged(&store, "k", "v").await.unwrap());
}
