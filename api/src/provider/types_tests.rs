//! Tests for the contract-only value types.
//!
//! The focus is the two things a later slice can silently break: the
//! fail-closed reading of an empty [`SourceScope`], and the wire strings /
//! serde defaults that an out-of-process driver depends on.

use super::*;

#[test]
fn empty_source_scope_denies_every_source() {
    let scope = SourceScope::default();
    assert!(scope.is_empty());
    assert!(!scope.allows_source_id("src-abc"));
    assert!(!scope.allows_source_id("mem_src:src-abc:item"));
}

#[test]
fn source_scope_matches_exact_id_and_mem_src_prefix() {
    let scope = SourceScope::new(["src-abc", "src-def"]);

    assert!(scope.allows_source_id("src-abc"));
    assert!(scope.allows_source_id("src-def"));
    assert!(scope.allows_source_id("mem_src:src-abc:item-1"));
    assert!(scope.allows_source_id("mem_src:src-def:nested:item"));

    assert!(!scope.allows_source_id("src-xyz"));
    assert!(!scope.allows_source_id("mem_src:src-xyz:item-1"));
}

#[test]
fn source_scope_prefix_requires_the_trailing_separator() {
    // `src-abc` must not smear onto `src-abcdef`: the engine's SQL binds
    // `mem_src:{id}:` including the trailing colon, so a longer id that merely
    // starts with an allowed one is out of scope.
    let scope = SourceScope::new(["src-abc"]);
    assert!(!scope.allows_source_id("mem_src:src-abcdef:item"));
    assert!(!scope.allows_source_id("src-abcdef"));
}

#[test]
fn change_kind_wire_strings_match_the_engine() {
    // These strings are shared with `memory::diff::types::ChangeKind`, so the
    // embedded adapter maps rather than translates. Changing one is a contract
    // major bump.
    for (kind, expected) in [
        (ChangeKind::Added, "added"),
        (ChangeKind::Removed, "removed"),
        (ChangeKind::Modified, "modified"),
    ] {
        assert_eq!(kind.as_str(), expected);
        assert_eq!(
            serde_json::to_value(kind).expect("serialize change kind"),
            serde_json::Value::String(expected.to_string()),
        );
    }
}

#[test]
fn export_page_terminates_on_absent_cursor_not_empty_records() {
    let page = ExportPage::default();
    assert!(page.records.is_empty());
    assert!(page.next_cursor.is_none());

    // An empty page with a cursor is a legitimate mid-export state, so callers
    // must not treat "no records" as the terminator.
    let midway = ExportPage {
        records: Vec::new(),
        next_cursor: Some("cursor-2".to_string()),
    };
    assert!(midway.next_cursor.is_some());
}

#[test]
fn export_record_round_trips_taint_and_opaque_payload() {
    let record = ExportRecord {
        kind: "vendor_specific_kind".to_string(),
        id: "rec-1".to_string(),
        namespace: Some("global".to_string()),
        taint: MemoryTaint::ExternalSync,
        payload: serde_json::json!({ "anything": [1, 2, 3] }),
    };

    let json = serde_json::to_string(&record).expect("serialize record");
    let back: ExportRecord = serde_json::from_str(&json).expect("deserialize record");

    assert_eq!(back, record);
    assert_eq!(back.taint, MemoryTaint::ExternalSync);
}

#[test]
fn ingest_item_deserializes_from_the_minimal_body() {
    // Every optional field carries `#[serde(default)]`, so a caller that knows
    // only source, id, and content can still build a valid request.
    let item: IngestItem = serde_json::from_value(serde_json::json!({
        "source": "notion",
        "source_id": "page-1",
        "content": "hello",
    }))
    .expect("deserialize minimal ingest item");

    assert_eq!(item.source, DataSource::Notion);
    assert_eq!(item.namespace, None);
    assert_eq!(item.owner, "");
    assert!(item.tags.is_empty());
    // Provenance defaults to the conservative-for-writes `Internal`; the host
    // guard overrides it explicitly on every sync path.
    assert_eq!(item.taint, MemoryTaint::Internal);
}

#[test]
fn maintenance_report_defaults_to_a_clean_read_only_run() {
    let report = MaintenanceReport {
        operation: "doctor".to_string(),
        ..MaintenanceReport::default()
    };
    assert_eq!(report.changed, 0);
    assert!(report.findings.is_empty());
}

#[test]
fn diff_report_expresses_a_first_ever_diff_without_a_sentinel() {
    let report = DiffReport {
        source_id: "src-abc".to_string(),
        from_snapshot_id: None,
        to_snapshot_id: "snap-1".to_string(),
        added: 3,
        ..DiffReport::default()
    };

    let json = serde_json::to_value(&report).expect("serialize diff report");
    assert_eq!(json["from_snapshot_id"], serde_json::Value::Null);
    assert_eq!(json["added"], 3);
}
