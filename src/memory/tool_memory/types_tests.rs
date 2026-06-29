//! Tests for the tool-scoped memory domain types.

use super::*;

#[test]
fn priority_default_is_normal() {
    assert_eq!(ToolMemoryPriority::default(), ToolMemoryPriority::Normal);
}

#[test]
fn priority_ordering_puts_critical_above_high() {
    assert!(ToolMemoryPriority::Critical > ToolMemoryPriority::High);
    assert!(ToolMemoryPriority::High > ToolMemoryPriority::Normal);
}

#[test]
fn priority_is_eager_for_high_and_critical_only() {
    assert!(ToolMemoryPriority::Critical.is_eager());
    assert!(ToolMemoryPriority::High.is_eager());
    assert!(!ToolMemoryPriority::Normal.is_eager());
}

#[test]
fn priority_snake_case_serde() {
    assert_eq!(
        serde_json::to_string(&ToolMemoryPriority::Critical).unwrap(),
        "\"critical\""
    );
    assert_eq!(
        serde_json::to_string(&ToolMemoryPriority::Normal).unwrap(),
        "\"normal\""
    );
}

#[test]
fn source_snake_case_serde() {
    assert_eq!(
        serde_json::to_string(&ToolMemorySource::UserExplicit).unwrap(),
        "\"user_explicit\""
    );
    assert_eq!(
        serde_json::to_string(&ToolMemorySource::PostTurn).unwrap(),
        "\"post_turn\""
    );
    assert_eq!(
        serde_json::to_string(&ToolMemorySource::Programmatic).unwrap(),
        "\"programmatic\""
    );
}

#[test]
fn source_default_is_programmatic() {
    assert_eq!(ToolMemorySource::default(), ToolMemorySource::Programmatic);
}

#[test]
fn rule_new_fills_id_and_timestamps() {
    let rule = ToolMemoryRule::new(
        "email",
        "never email Sarah",
        ToolMemoryPriority::Critical,
        ToolMemorySource::UserExplicit,
    );
    assert!(!rule.id.is_empty());
    assert_eq!(rule.tool_name, "email");
    assert_eq!(rule.rule, "never email Sarah");
    assert_eq!(rule.priority, ToolMemoryPriority::Critical);
    assert_eq!(rule.source, ToolMemorySource::UserExplicit);
    assert!(rule.created_at == rule.updated_at);
}

#[test]
fn rule_generate_id_produces_unique_values() {
    let a = ToolMemoryRule::generate_id();
    let b = ToolMemoryRule::generate_id();
    assert_ne!(a, b);
    assert!(a.starts_with('r'));
    assert!(a[1..].chars().all(|c| matches!(c, 'a'..='p')));
}

#[test]
fn generated_rule_ids_are_safe_memory_document_keys() {
    // Generated ids must be free of digits and separators so the resulting
    // storage key never resembles PII (phone numbers, ids, etc.) to a
    // boundary check downstream.
    for _ in 0..128 {
        let id = ToolMemoryRule::generate_id();
        assert!(
            id.chars().all(|ch| ch.is_ascii_lowercase()),
            "generated id should avoid PII-shaped digits and separators: {id}"
        );
        let key = ToolMemoryRule::storage_key(&id);
        assert!(
            key.bytes().all(|b| b == b'/' || b.is_ascii_lowercase()),
            "generated storage key should not contain PII-shaped bytes: {key}"
        );
    }
}

#[test]
fn rule_storage_key_uses_rule_prefix() {
    assert_eq!(ToolMemoryRule::storage_key("abc"), "rule/abc");
}

#[test]
fn rule_serde_roundtrip_preserves_fields() {
    let rule = ToolMemoryRule {
        id: "id-1".into(),
        tool_name: "shell".into(),
        rule: "never run sudo".into(),
        priority: ToolMemoryPriority::High,
        source: ToolMemorySource::PostTurn,
        tags: vec!["safety".into()],
        created_at: "2026-05-11T00:00:00Z".into(),
        updated_at: "2026-05-11T00:00:01Z".into(),
    };
    let json = serde_json::to_string(&rule).unwrap();
    let back: ToolMemoryRule = serde_json::from_str(&json).unwrap();
    assert_eq!(back, rule);
}

#[test]
fn namespace_uses_tool_prefix_and_trims_whitespace() {
    assert_eq!(tool_memory_namespace("email"), "tool-email");
    assert_eq!(tool_memory_namespace("  shell  "), "tool-shell");
    assert_eq!(tool_memory_namespace("Send_Email"), "tool-send_email");
    assert_eq!(tool_memory_namespace("WebSearch"), "tool-websearch");
}
