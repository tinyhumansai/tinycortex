//! Tests for markdown composition. Ported from OpenHuman's inline
//! `memory_archivist::compose` tests.

use super::*;
use chrono::Utc;

fn t(role: &str, content: &str) -> Turn {
    Turn {
        role: role.into(),
        content: content.into(),
        tool_calls_json: None,
        timestamp: Utc::now(),
    }
}

#[test]
fn empty_input_gives_empty_string() {
    assert_eq!(compose_conversation_md(&[]), "");
}

#[test]
fn role_headings_separate_turns() {
    let md = compose_conversation_md(&[t("user", "hi"), t("assistant", "hello")]);
    assert!(md.contains("## user\nhi\n"));
    assert!(md.contains("## assistant\nhello\n"));
}

#[test]
fn turns_separated_by_blank_line() {
    let md = compose_conversation_md(&[t("user", "a"), t("user", "b")]);
    // turn boundaries get one blank line between them
    assert!(md.contains("a\n\n## user\nb"));
}
