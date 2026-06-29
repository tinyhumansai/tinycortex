//! Tests for archivist input shapes. Ported from OpenHuman's inline
//! `memory_archivist::types` tests.

use super::*;
use serde_json::json;

#[test]
fn archived_turn_defaults_are_empty_and_zero() {
    let turn = ArchivedTurn::default();
    assert!(turn.session_id.is_empty());
    assert_eq!(turn.seq, 0);
    assert_eq!(turn.timestamp_ms, 0);
    assert!(turn.role.is_empty());
    assert!(turn.content.is_empty());
    assert!(turn.lesson.is_none());
    assert!(turn.tool_calls_json.is_none());
    assert_eq!(turn.cost_microdollars, 0);
}

#[test]
fn turn_new_sets_role_content_and_no_tool_calls() {
    let turn = Turn::new("user", "hello");
    assert_eq!(turn.role, "user");
    assert_eq!(turn.content, "hello");
    assert!(turn.tool_calls_json.is_none());
}

#[test]
fn archived_turn_serde_skips_absent_optional_fields() {
    let turn = ArchivedTurn {
        session_id: "s1".into(),
        seq: 1,
        timestamp_ms: 123,
        role: "assistant".into(),
        content: "done".into(),
        lesson: None,
        tool_calls_json: None,
        cost_microdollars: 55,
    };
    let value = serde_json::to_value(&turn).unwrap();
    assert_eq!(value["session_id"], json!("s1"));
    assert!(value.get("lesson").is_none());
    assert!(value.get("tool_calls_json").is_none());

    let decoded: ArchivedTurn = serde_json::from_value(value).unwrap();
    assert_eq!(decoded, turn);
}
