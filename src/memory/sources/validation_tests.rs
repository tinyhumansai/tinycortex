//! Tests for required-field validation and the path-traversal guard.

use super::*;
use crate::memory::sources::types::SourceKind;
use std::fs;
use tempfile::TempDir;

fn entry(kind: SourceKind) -> MemorySourceEntry {
    MemorySourceEntry {
        id: "src_x".into(),
        kind,
        label: "Label".into(),
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
fn empty_id_or_label_is_rejected_for_every_kind() {
    let mut e = entry(SourceKind::Conversation);
    e.id = String::new();
    assert!(validate_entry(&e).is_err());

    let mut e = entry(SourceKind::Conversation);
    e.label = String::new();
    assert!(validate_entry(&e).is_err());
}

#[test]
fn empty_string_field_counts_as_missing() {
    let mut e = entry(SourceKind::Folder);
    e.path = Some(String::new());
    assert!(validate_entry(&e).is_err());
}

#[test]
fn composio_requires_both_toolkit_and_connection() {
    let mut e = entry(SourceKind::Composio);
    e.toolkit = Some("gmail".into());
    assert!(validate_entry(&e).is_err());
    e.connection_id = Some("conn".into());
    assert!(validate_entry(&e).is_ok());
}

#[test]
fn ensure_within_base_accepts_contained_file() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("ok.md"), "hi").unwrap();
    let resolved = ensure_within_base(tmp.path(), &tmp.path().join("ok.md")).unwrap();
    assert!(resolved.ends_with("ok.md"));
}

#[test]
fn ensure_within_base_rejects_escape() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("ok.md"), "hi").unwrap();
    // Build a target that escapes the base via `..`.
    let escaping = tmp.path().join("../../etc/hosts");
    let result = ensure_within_base(tmp.path(), &escaping);
    assert!(result.is_err());
}
