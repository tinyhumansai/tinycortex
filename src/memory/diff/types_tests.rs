//! Serde round-trip and default tests for the diff domain types. Ported from
//! OpenHuman `memory_diff::types` inline tests.

use super::*;

#[test]
fn snapshot_trigger_round_trips() {
    for trigger in [SnapshotTrigger::Auto, SnapshotTrigger::Manual] {
        let json = serde_json::to_string(&trigger).unwrap();
        let decoded: SnapshotTrigger = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, trigger);
    }
}

#[test]
fn snapshot_trigger_wire_strings() {
    assert_eq!(
        serde_json::to_string(&SnapshotTrigger::Auto).unwrap(),
        "\"auto\""
    );
    assert_eq!(
        serde_json::to_string(&SnapshotTrigger::Manual).unwrap(),
        "\"manual\""
    );
    assert_eq!(SnapshotTrigger::Auto.as_str(), "auto");
    assert_eq!(SnapshotTrigger::Manual.as_str(), "manual");
}

#[test]
fn change_kind_round_trips() {
    for kind in [ChangeKind::Added, ChangeKind::Removed, ChangeKind::Modified] {
        let json = serde_json::to_string(&kind).unwrap();
        let decoded: ChangeKind = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, kind);
    }
}

#[test]
fn change_kind_wire_strings() {
    assert_eq!(
        serde_json::to_string(&ChangeKind::Added).unwrap(),
        "\"added\""
    );
    assert_eq!(
        serde_json::to_string(&ChangeKind::Removed).unwrap(),
        "\"removed\""
    );
    assert_eq!(
        serde_json::to_string(&ChangeKind::Modified).unwrap(),
        "\"modified\""
    );
}

#[test]
fn diff_summary_defaults_to_zero() {
    let s = DiffSummary::default();
    assert_eq!(s.added, 0);
    assert_eq!(s.removed, 0);
    assert_eq!(s.modified, 0);
    assert_eq!(s.unchanged, 0);
}

#[test]
fn item_change_omits_absent_optionals() {
    let change = ItemChange {
        item_id: "a".into(),
        title: "Alpha".into(),
        kind: ChangeKind::Added,
        old_content_hash: None,
        new_content_hash: Some("deadbeef".into()),
        text_diff: None,
    };
    let json = serde_json::to_value(&change).unwrap();
    assert!(json.get("old_content_hash").is_none());
    assert!(json.get("text_diff").is_none());
    assert_eq!(json["new_content_hash"], "deadbeef");
}
