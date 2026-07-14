//! Tests for PersonaConfig defaults, serde, and ask resolution.

use super::*;
use std::path::Path;

#[test]
fn with_home_sets_platform_defaults() {
    let cfg = PersonaConfig::with_home(Path::new("/home/x"), "x@example.com");
    assert_eq!(
        cfg.claude_code_root.as_deref(),
        Some(Path::new("/home/x/.claude/projects"))
    );
    assert_eq!(
        cfg.codex_root.as_deref(),
        Some(Path::new("/home/x/.codex/sessions"))
    );
    assert!(cfg
        .global_instruction_files
        .iter()
        .any(|p| p.ends_with(".claude/CLAUDE.md")));
    assert!(cfg
        .global_instruction_files
        .iter()
        .any(|p| p.ends_with(".codex/AGENTS.md")));
    assert_eq!(cfg.total_token_budget, DEFAULT_TOTAL_MAX);
}

#[test]
fn asks_fall_back_to_defaults_and_honour_overrides() {
    let mut cfg = PersonaConfig::with_home(Path::new("/home/x"), "x");
    cfg.facet_asks
        .insert("workflow".to_string(), "custom workflow ask".to_string());
    let asks = cfg.asks();
    assert_eq!(asks.ask(PersonaFacet::Workflow), "custom workflow ask");
    // Un-overridden facet uses the built-in default.
    assert_eq!(
        asks.ask(PersonaFacet::CodingStyle),
        PersonaFacet::CodingStyle.default_ask()
    );
}

#[test]
fn round_trips_through_json() {
    let cfg = PersonaConfig::with_home(Path::new("/home/x"), "x@example.com");
    let json = serde_json::to_string(&cfg).unwrap();
    let back: PersonaConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(back.identity, "x@example.com");
    assert_eq!(back.chat_model, cfg.chat_model);
    assert_eq!(back.run_budget.max_sessions, cfg.run_budget.max_sessions);
}
