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
async fn read_item_enforces_configured_glob() {
    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("docs")).unwrap();
    fs::write(tmp.path().join("docs/allowed.md"), "allowed").unwrap();
    fs::write(tmp.path().join("docs/secret.env"), "secret").unwrap();
    let mut source = folder_source(&tmp.path().to_string_lossy());
    source.glob = Some("docs/**/*.md".into());
    let reader = FolderReader;

    assert!(reader
        .read_item(&source, "docs/allowed.md", &config())
        .await
        .is_ok());
    let err = reader
        .read_item(&source, "docs/secret.env", &config())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("outside source glob"));
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

// ── openhuman#5830: relative paths resolve against the workspace ─────────────

/// Build a workspace containing `relative_docs/note.md`, and a `MemoryConfig`
/// rooted at it.
///
/// The subdirectory name is deliberately distinctive: a relative path resolves
/// against the process CWD before this fix, and the CWD under `cargo test` is
/// the crate directory — a common name like `docs` could accidentally exist
/// there and make the pre-fix run pass for the wrong reason.
fn workspace_with_relative_folder() -> (TempDir, MemoryConfig) {
    let tmp = TempDir::new().unwrap();
    let nested = tmp.path().join("relative_docs");
    fs::create_dir_all(&nested).unwrap();
    fs::write(nested.join("note.md"), "# note").unwrap();
    let cfg = MemoryConfig::new(tmp.path());
    (tmp, cfg)
}

/// A relative folder path must be anchored on the memory workspace, not on
/// whatever directory the host process happened to start in.
///
/// This is openhuman#5830: the desktop app's CWD is the Tauri build directory,
/// so a source configured as `docs` looked in `…/app/src-tauri/docs` and failed
/// on every sync cycle, permanently, for a source that could work.
#[tokio::test]
async fn list_items_resolves_a_relative_path_against_the_workspace() {
    let (_tmp, cfg) = workspace_with_relative_folder();
    let source = folder_source("relative_docs");
    let reader = FolderReader;

    let items = reader
        .list_items(&source, &cfg)
        .await
        .expect("a relative folder path must resolve against the workspace, not the process CWD");

    assert_eq!(
        items.len(),
        1,
        "the workspace-relative folder holds exactly one .md file"
    );
    assert_eq!(items[0].id, "note.md");
}

/// `read_item` must resolve by the same rule as `list_items`.
///
/// Fixing only the listing half would be worse than the original bug: the
/// reader would walk the workspace and then read back from the CWD, so every
/// item it just listed would fail to load.
#[tokio::test]
async fn read_item_resolves_a_relative_path_against_the_workspace() {
    let (_tmp, cfg) = workspace_with_relative_folder();
    let source = folder_source("relative_docs");
    let reader = FolderReader;

    let content = reader
        .read_item(&source, "note.md", &cfg)
        .await
        .expect("read_item must resolve a relative path against the workspace, like list_items");

    assert_eq!(content.body, "# note");
}

/// The error has to say **where the reader looked**, not only what it was
/// configured with. `folder does not exist: docs` is what cost a source-read
/// and an `lsof` of the running process to diagnose.
#[tokio::test]
async fn a_missing_relative_folder_error_names_the_resolved_path() {
    let tmp = TempDir::new().unwrap();
    let cfg = MemoryConfig::new(tmp.path());
    let source = folder_source("relative_docs");
    let reader = FolderReader;

    let err = reader
        .list_items(&source, &cfg)
        .await
        .expect_err("a missing folder is still an error")
        .to_string();

    assert!(
        err.contains("resolved to"),
        "the error must name the resolved path, not only the configured one: {err}"
    );
    assert!(
        err.contains(&tmp.path().join("relative_docs").display().to_string()),
        "the resolved path must be the workspace-anchored one: {err}"
    );
}

/// An absolute path keeps working exactly as before, and the workspace must not
/// influence it — otherwise this fix would break every source already
/// configured with an absolute path.
#[tokio::test]
async fn an_absolute_path_ignores_the_workspace() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("note.md"), "# note").unwrap();
    let source = folder_source(&tmp.path().to_string_lossy());
    let reader = FolderReader;

    // A workspace that does not exist and shares no prefix with the source: if
    // the absolute path were being joined onto it, nothing would resolve.
    let bogus = MemoryConfig::new("/nonexistent/workspace/root");
    let items = reader
        .list_items(&source, &bogus)
        .await
        .expect("an absolute folder path must resolve without consulting the workspace");

    assert_eq!(items.len(), 1, "the absolute folder still lists its file");
}

/// For an absolute path the configured string *is* the resolved path, so the
/// error must not echo it twice.
#[tokio::test]
async fn an_absolute_missing_folder_error_does_not_echo_itself() {
    let source = folder_source("/nonexistent/path/xyz");
    let reader = FolderReader;

    let err = reader
        .list_items(&source, &config())
        .await
        .expect_err("a missing folder is still an error")
        .to_string();

    assert!(
        !err.contains("resolved to"),
        "an absolute path is already resolved; the error must not repeat it: {err}"
    );
}
