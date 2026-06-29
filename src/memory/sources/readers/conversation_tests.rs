//! Tests for the conversation reader.

use super::*;
use crate::memory::config::MemoryConfig;
use std::fs;
use tempfile::tempdir;

fn conversation_source() -> MemorySourceEntry {
    MemorySourceEntry {
        id: "src_conv".into(),
        kind: SourceKind::Conversation,
        label: "Conversations".into(),
        enabled: true,
        toolkit: None,
        connection_id: None,
        path: None,
        glob: None,
        url: None,
        branch: None,
        paths: Vec::new(),
        max_commits: None,
        max_issues: None,
        max_prs: None,
        query: None,
        since_days: None,
        max_items: None,
        selector: None,
        max_tokens_per_sync: None,
        max_cost_per_sync_usd: None,
        sync_depth_days: None,
    }
}

#[test]
fn format_thread_produces_markdown() {
    let thread = serde_json::json!({
        "title": "Test chat",
        "messages": [
            {"role": "user", "content": "Hello"},
            {"role": "assistant", "content": "Hi there!"},
        ]
    });
    let md = format_thread_as_markdown(&thread);
    assert!(md.contains("# Test chat"));
    assert!(md.contains("**user**: Hello"));
    assert!(md.contains("**assistant**: Hi there!"));
}

#[test]
fn format_thread_skips_empty_content() {
    let thread = serde_json::json!({
        "title": "Sparse",
        "messages": [
            {"role": "user", "content": ""},
            {"role": "assistant", "content": "Reply"},
            {"role": "user", "content": ""},
        ]
    });
    let md = format_thread_as_markdown(&thread);
    assert!(!md.contains("**user**:"));
    assert!(md.contains("**assistant**: Reply"));
}

#[test]
fn format_thread_handles_missing_title() {
    let thread = serde_json::json!({
        "messages": [{"role": "user", "content": "Hi"}]
    });
    let md = format_thread_as_markdown(&thread);
    assert!(!md.starts_with('#'));
    assert!(md.contains("**user**: Hi"));
}

#[test]
fn format_thread_handles_no_messages() {
    let thread = serde_json::json!({"title": "Empty"});
    let md = format_thread_as_markdown(&thread);
    assert!(md.contains("# Empty"));
    assert_eq!(md.trim(), "# Empty");
}

#[tokio::test]
async fn list_items_returns_empty_when_no_threads_dir() {
    let tmp = tempdir().unwrap();
    let config = MemoryConfig::new(tmp.path());

    let source = conversation_source();
    let reader = ConversationReader;
    let items = reader.list_items(&source, &config).await.unwrap();
    assert!(items.is_empty());
}

#[tokio::test]
async fn list_items_finds_json_thread_files() {
    let tmp = tempdir().unwrap();
    let threads_dir = tmp.path().join("threads");
    fs::create_dir_all(&threads_dir).unwrap();

    fs::write(
        threads_dir.join("thread_abc.json"),
        r#"{"title":"Chat 1","messages":[]}"#,
    )
    .unwrap();
    fs::write(
        threads_dir.join("thread_def.json"),
        r#"{"title":"Chat 2","messages":[]}"#,
    )
    .unwrap();
    // Non-json file should be ignored.
    fs::write(threads_dir.join("notes.txt"), "ignored").unwrap();

    let config = MemoryConfig::new(tmp.path());
    let source = conversation_source();
    let reader = ConversationReader;
    let items = reader.list_items(&source, &config).await.unwrap();
    assert_eq!(items.len(), 2);

    let ids: Vec<&str> = items.iter().map(|i| i.id.as_str()).collect();
    assert!(ids.contains(&"thread_abc"));
    assert!(ids.contains(&"thread_def"));
}

#[tokio::test]
async fn read_item_returns_formatted_content() {
    let tmp = tempdir().unwrap();
    let threads_dir = tmp.path().join("threads");
    fs::create_dir_all(&threads_dir).unwrap();

    let thread_json = serde_json::json!({
        "title": "Test Conversation",
        "messages": [
            {"role": "user", "content": "What is 2+2?"},
            {"role": "assistant", "content": "4"},
        ]
    });
    fs::write(
        threads_dir.join("conv_123.json"),
        serde_json::to_string(&thread_json).unwrap(),
    )
    .unwrap();

    let config = MemoryConfig::new(tmp.path());
    let source = conversation_source();
    let reader = ConversationReader;
    let content = reader
        .read_item(&source, "conv_123", &config)
        .await
        .unwrap();

    assert_eq!(content.id, "conv_123");
    assert_eq!(content.title, "Test Conversation");
    assert_eq!(content.content_type, ContentType::Markdown);
    assert!(content.body.contains("**user**: What is 2+2?"));
    assert!(content.body.contains("**assistant**: 4"));
}

#[tokio::test]
async fn read_item_returns_error_for_missing_thread() {
    let tmp = tempdir().unwrap();
    let threads_dir = tmp.path().join("threads");
    fs::create_dir_all(&threads_dir).unwrap();

    let config = MemoryConfig::new(tmp.path());
    let source = conversation_source();
    let reader = ConversationReader;
    let result = reader.read_item(&source, "nonexistent", &config).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("not found"));
}

#[tokio::test]
async fn read_item_rejects_path_traversal() {
    let tmp = tempdir().unwrap();
    let threads_dir = tmp.path().join("threads");
    fs::create_dir_all(&threads_dir).unwrap();

    let config = MemoryConfig::new(tmp.path());
    let source = conversation_source();
    let reader = ConversationReader;

    let result = reader.read_item(&source, "../config", &config).await;
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("path traversal denied"));

    let result = reader
        .read_item(&source, "foo/../../etc/passwd", &config)
        .await;
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("path traversal denied"));
}
