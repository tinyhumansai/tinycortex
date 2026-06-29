//! Tests for the local folder reader.

use super::*;
use crate::memory::config::MemoryConfig;
use std::fs;
use tempfile::TempDir;

fn folder_source(path: &str) -> MemorySourceEntry {
    MemorySourceEntry {
        id: "src_folder".into(),
        kind: SourceKind::Folder,
        label: "Test folder".into(),
        enabled: true,
        toolkit: None,
        connection_id: None,
        path: Some(path.into()),
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

fn config() -> MemoryConfig {
    MemoryConfig::new("/unused")
}

#[test]
fn glob_to_regex_matches_default_pattern() {
    let re = glob_to_regex("**/*.md").unwrap();
    assert!(re.is_match("note.md"));
    assert!(re.is_match("sub/dir/note.md"));
    assert!(!re.is_match("note.txt"));
}

#[test]
fn glob_to_regex_single_star_excludes_separators() {
    let re = glob_to_regex("*.md").unwrap();
    assert!(re.is_match("note.md"));
    assert!(!re.is_match("sub/note.md"));
}

#[tokio::test]
async fn list_items_finds_md_files() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("note.md"), "# Hello").unwrap();
    fs::write(tmp.path().join("data.txt"), "ignored").unwrap();

    let source = folder_source(&tmp.path().to_string_lossy());
    let reader = FolderReader;
    let items = reader.list_items(&source, &config()).await.unwrap();

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "note.md");
}

#[tokio::test]
async fn list_items_recurses_into_subdirectories() {
    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("sub")).unwrap();
    fs::write(tmp.path().join("top.md"), "a").unwrap();
    fs::write(tmp.path().join("sub/nested.md"), "b").unwrap();

    let source = folder_source(&tmp.path().to_string_lossy());
    let reader = FolderReader;
    let items = reader.list_items(&source, &config()).await.unwrap();

    let ids: Vec<&str> = items.iter().map(|i| i.id.as_str()).collect();
    assert_eq!(items.len(), 2);
    assert!(ids.contains(&"top.md"));
    assert!(ids.contains(&"sub/nested.md"));
}

#[tokio::test]
async fn read_item_returns_file_content() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("test.md"), "# Test\nBody").unwrap();

    let source = folder_source(&tmp.path().to_string_lossy());
    let reader = FolderReader;
    let content = reader
        .read_item(&source, "test.md", &config())
        .await
        .unwrap();

    assert_eq!(content.body, "# Test\nBody");
    assert_eq!(content.content_type, ContentType::Markdown);
}

#[tokio::test]
async fn read_item_prevents_path_traversal() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("safe.md"), "ok").unwrap();

    let source = folder_source(&tmp.path().to_string_lossy());
    let reader = FolderReader;
    let result = reader
        .read_item(&source, "../../../etc/passwd", &config())
        .await;

    assert!(result.is_err());
}

#[tokio::test]
async fn list_items_nonexistent_folder_errors() {
    let source = folder_source("/nonexistent/path/xyz");
    let reader = FolderReader;
    let result = reader.list_items(&source, &config()).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn read_item_missing_file_errors() {
    let tmp = TempDir::new().unwrap();
    let source = folder_source(&tmp.path().to_string_lossy());
    let reader = FolderReader;
    let result = reader.read_item(&source, "missing.md", &config()).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("not found"));
}
