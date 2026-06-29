//! Tests for tool-scoped memory prompt rendering.

use super::*;
use crate::memory::tool_memory::types::ToolMemorySource;

fn rule(tool: &str, body: &str, priority: ToolMemoryPriority) -> ToolMemoryRule {
    ToolMemoryRule {
        id: format!("{tool}/{body}"),
        tool_name: tool.into(),
        rule: body.into(),
        priority,
        source: ToolMemorySource::UserExplicit,
        tags: vec![],
        created_at: "2026-05-11T00:00:00Z".into(),
        updated_at: "2026-05-11T00:00:00Z".into(),
    }
}

#[test]
fn renders_empty_when_no_rules() {
    assert!(render_tool_memory_rules(&[]).is_empty());
}

#[test]
fn section_empty_returns_blank_output() {
    let section = ToolMemoryRulesSection::empty();
    assert!(section.is_empty());
    assert!(section.rendered().is_empty());
}

#[test]
fn renders_heading_and_priority_markers() {
    let rules = vec![
        rule("email", "never email Sarah", ToolMemoryPriority::Critical),
        rule("shell", "avoid sudo", ToolMemoryPriority::High),
    ];
    let out = render_tool_memory_rules(&rules);
    assert!(out.contains(TOOL_MEMORY_HEADING));
    assert!(out.contains("### `email`"));
    assert!(out.contains("### `shell`"));
    assert!(out.contains("**[critical]**"));
    assert!(out.contains("**[high]**"));
    assert!(out.contains("never email Sarah"));
    assert!(out.contains("avoid sudo"));
}

#[test]
fn renders_critical_before_high_regardless_of_input_order() {
    let rules = vec![
        rule("shell", "avoid sudo", ToolMemoryPriority::High),
        rule("email", "never email Sarah", ToolMemoryPriority::Critical),
    ];
    let out = render_tool_memory_rules(&rules);
    let critical_pos = out.find("never email Sarah").unwrap();
    let high_pos = out.find("avoid sudo").unwrap();
    assert!(
        critical_pos < high_pos,
        "Critical rules must render before High; output:\n{out}"
    );
}

#[test]
fn renders_byte_stable_output_for_identical_inputs() {
    let rules = vec![
        rule("email", "never email Sarah", ToolMemoryPriority::Critical),
        rule("shell", "avoid sudo", ToolMemoryPriority::High),
    ];
    let first = render_tool_memory_rules(&rules);
    let again = render_tool_memory_rules(&rules);
    assert_eq!(first, again);
}

#[test]
fn section_renders_snapshot_via_rendered_accessor() {
    let section = ToolMemoryRulesSection::new(vec![rule(
        "email",
        "never email Sarah",
        ToolMemoryPriority::Critical,
    )]);
    assert!(!section.is_empty());
    assert!(section.rendered().contains("never email Sarah"));
}
