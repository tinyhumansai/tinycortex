//! Tests for the TOML-backed source registry.

use super::*;
use crate::memory::sources::types::SourceKind;
use tempfile::TempDir;

fn registry() -> (TempDir, SourceRegistry) {
    let tmp = TempDir::new().unwrap();
    let reg = SourceRegistry::new(tmp.path().join("config.toml"));
    (tmp, reg)
}

fn folder_entry(id: &str) -> MemorySourceEntry {
    let mut e = MemorySourceEntry {
        id: id.into(),
        kind: SourceKind::Folder,
        label: "Notes".into(),
        enabled: true,
        toolkit: None,
        connection_id: None,
        path: Some("/tmp/notes".into()),
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
    };
    e.glob = Some("**/*.md".into());
    e
}

#[test]
fn list_is_empty_for_missing_file() {
    let (_tmp, reg) = registry();
    assert!(reg.list().unwrap().is_empty());
    assert!(reg.get("anything").unwrap().is_none());
}

#[test]
fn add_get_list_round_trip() {
    let (_tmp, reg) = registry();
    let added = reg.add(folder_entry("src_1")).unwrap();
    assert_eq!(added.id, "src_1");

    let got = reg.get("src_1").unwrap().unwrap();
    assert_eq!(got.kind, SourceKind::Folder);
    assert_eq!(got.path.as_deref(), Some("/tmp/notes"));

    let all = reg.list().unwrap();
    assert_eq!(all.len(), 1);
}

#[test]
fn add_rejects_duplicate_id() {
    let (_tmp, reg) = registry();
    reg.add(folder_entry("src_dup")).unwrap();
    assert!(reg.add(folder_entry("src_dup")).is_err());
}

#[test]
fn add_rejects_invalid_entry() {
    let (_tmp, reg) = registry();
    let mut bad = folder_entry("src_bad");
    bad.path = None; // folder requires a path
    assert!(reg.add(bad).is_err());
    assert!(reg.list().unwrap().is_empty());
}

#[test]
fn update_applies_patch_and_persists() {
    let (_tmp, reg) = registry();
    reg.add(folder_entry("src_u")).unwrap();

    let patch = MemorySourcePatch {
        label: Some("Renamed".into()),
        enabled: Some(false),
        ..Default::default()
    };
    let updated = reg.update("src_u", patch).unwrap();
    assert_eq!(updated.label, "Renamed");
    assert!(!updated.enabled);

    // Re-read from disk to confirm persistence.
    let got = reg.get("src_u").unwrap().unwrap();
    assert_eq!(got.label, "Renamed");
    assert!(!got.enabled);
}

#[test]
fn update_missing_id_errors() {
    let (_tmp, reg) = registry();
    assert!(reg.update("nope", MemorySourcePatch::default()).is_err());
}

#[test]
fn remove_returns_whether_anything_was_removed() {
    let (_tmp, reg) = registry();
    reg.add(folder_entry("src_r")).unwrap();
    assert!(reg.remove("src_r").unwrap());
    assert!(!reg.remove("src_r").unwrap());
    assert!(reg.list().unwrap().is_empty());
}

#[test]
fn list_enabled_by_kind_filters() {
    let (_tmp, reg) = registry();
    reg.add(folder_entry("src_a")).unwrap();
    let mut disabled = folder_entry("src_b");
    disabled.enabled = false;
    reg.add(disabled).unwrap();

    let enabled = reg.list_enabled_by_kind(SourceKind::Folder).unwrap();
    assert_eq!(enabled.len(), 1);
    assert_eq!(enabled[0].id, "src_a");
    assert!(reg
        .list_enabled_by_kind(SourceKind::Conversation)
        .unwrap()
        .is_empty());
}

#[test]
fn write_preserves_other_top_level_config_keys() {
    let (tmp, reg) = registry();
    let path = tmp.path().join("config.toml");
    std::fs::write(&path, "workspace = \"/data\"\n").unwrap();

    reg.add(folder_entry("src_keep")).unwrap();

    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("workspace = \"/data\""));
    assert!(text.contains("[[memory_sources]]"));
}

// ── Composio upsert ──

#[test]
fn composio_defaults_for_known_and_unknown_toolkits() {
    assert_eq!(
        memory_sync_defaults_for_toolkit("gmail"),
        (Some(100), Some(30))
    );
    assert_eq!(
        memory_sync_defaults_for_toolkit("slack"),
        (Some(50), Some(14))
    );
    assert_eq!(
        memory_sync_defaults_for_toolkit("unknown_xyz"),
        (Some(30), Some(14))
    );
}

#[test]
fn in_place_upsert_inserts_then_updates_label_only() {
    let mut sources: Vec<MemorySourceEntry> = vec![];
    let (entry, was_insert) =
        upsert_composio_entry_in_place(&mut sources, "gmail", "conn_a", "Gmail · conn_a");
    assert!(was_insert);
    assert_eq!(entry.toolkit.as_deref(), Some("gmail"));
    assert_eq!(entry.max_items, Some(100));
    assert_eq!(entry.sync_depth_days, Some(30));

    // User customises a cap, then a second upsert updates label only.
    sources[0].max_items = Some(7);
    let (entry, was_insert) =
        upsert_composio_entry_in_place(&mut sources, "gmail", "conn_a", "new label");
    assert!(!was_insert);
    assert_eq!(sources.len(), 1);
    assert_eq!(entry.label, "new label");
    assert_eq!(entry.max_items, Some(7));
}

#[test]
fn upsert_composio_source_persists_and_disconnect_removes() {
    let (_tmp, reg) = registry();
    reg.upsert_composio_source("gmail", "conn_a", "Gmail")
        .unwrap();
    reg.upsert_composio_source("slack", "conn_b", "Slack")
        .unwrap();
    assert_eq!(reg.list().unwrap().len(), 2);

    let removed = reg
        .remove_composio_source_by_connection_id("conn_a")
        .unwrap();
    assert_eq!(removed, 1);
    assert_eq!(reg.list().unwrap().len(), 1);
}

#[test]
fn apply_all_in_enables_and_clears_caps() {
    let (_tmp, reg) = registry();
    let mut capped = folder_entry("src_capped");
    capped.enabled = false;
    capped.max_items = Some(5);
    capped.sync_depth_days = Some(3);
    reg.add(capped).unwrap();

    let updated = reg.apply_all_in().unwrap();
    assert_eq!(updated.len(), 1);
    assert!(updated[0].enabled);
    assert!(updated[0].max_items.is_none());
    assert!(updated[0].sync_depth_days.is_none());
}

#[test]
fn memory_source_patch_deserializes_partial_and_github_fields() {
    let json = serde_json::json!({
        "label": "New label",
        "enabled": false,
        "max_commits": 100,
        "max_issues": 50,
        "max_prs": 25
    });
    let patch: MemorySourcePatch = serde_json::from_value(json).unwrap();
    assert_eq!(patch.label.as_deref(), Some("New label"));
    assert_eq!(patch.enabled, Some(false));
    assert_eq!(patch.max_commits, Some(100));
    assert_eq!(patch.max_issues, Some(50));
    assert_eq!(patch.max_prs, Some(25));
    assert!(patch.toolkit.is_none());
}
