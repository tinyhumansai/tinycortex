//! Unit tests for [`super::MemoryConfig`] and friends.

use super::*;

#[test]
fn default_config_uses_openhuman_constants() {
    let cfg = MemoryConfig::new("/tmp/ws");
    assert_eq!(cfg.embedding.dim, 768);
    assert_eq!(cfg.embedding.model, "nomic-embed-text");
    assert!(!cfg.embedding.strict);
    assert_eq!(cfg.tree.input_token_budget, 50_000);
    assert_eq!(cfg.tree.output_token_budget, 5_000);
    assert_eq!(cfg.tree.summary_fanout, 10);
    assert_eq!(cfg.tree.flush_age_secs, 604_800);
    assert_eq!(cfg.retrieval.default_profile, WeightProfile::BALANCED);
}

#[test]
fn weight_profiles_match_spec() {
    assert_eq!(WeightProfile::by_name("balanced"), WeightProfile::BALANCED);
    assert_eq!(WeightProfile::by_name("semantic"), WeightProfile::SEMANTIC);
    assert_eq!(WeightProfile::by_name("lexical"), WeightProfile::LEXICAL);
    assert_eq!(
        WeightProfile::by_name("graph_first"),
        WeightProfile::GRAPH_FIRST
    );
    // Unknown falls back to balanced.
    assert_eq!(WeightProfile::by_name("nope"), WeightProfile::BALANCED);

    let b = WeightProfile::BALANCED;
    assert!((b.graph + b.vector + b.keyword + b.freshness - 1.0).abs() < 1e-9);
}

#[test]
fn config_roundtrips_through_json() {
    let cfg = MemoryConfig::new("/tmp/ws");
    let json = serde_json::to_string(&cfg).unwrap();
    let parsed: MemoryConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.workspace, cfg.workspace);
    assert_eq!(parsed.tree.summary_fanout, cfg.tree.summary_fanout);
}

#[test]
fn config_fills_defaults_for_absent_sections() {
    // A host may supply only the workspace; every other section defaults.
    let parsed: MemoryConfig = serde_json::from_str(r#"{"workspace":"/tmp/ws"}"#).unwrap();
    assert_eq!(parsed.embedding.dim, 768);
    assert_eq!(parsed.tree.summary_fanout, 10);
    assert!(parsed.sync_budget.max_tokens_per_sync.is_none());
}
