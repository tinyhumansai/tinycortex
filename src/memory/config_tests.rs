//! Unit tests for [`super::MemoryConfig`] and friends.

use super::*;

#[test]
fn sync_secrets_are_redacted_and_not_serialized() {
    let secret = SecretString::new("super-secret");
    assert_eq!(secret.to_string(), "[REDACTED]");
    assert!(!format!("{secret:?}").contains("super-secret"));

    let config = ComposioSyncConfig {
        mode: ComposioMode::Direct,
        base_url: "https://backend.composio.dev".into(),
        api_key: Some(secret),
        bearer_token: Some(SecretString::new("bearer-secret")),
        entity_id: Some("entity".into()),
    };
    let json = serde_json::to_string(&config).unwrap();
    assert!(!json.contains("super-secret"));
    assert!(!json.contains("bearer-secret"));
}

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
    assert_eq!(
        WeightProfile::by_name("balanced"),
        Some(WeightProfile::BALANCED)
    );
    assert_eq!(
        WeightProfile::by_name("semantic"),
        Some(WeightProfile::SEMANTIC)
    );
    assert_eq!(
        WeightProfile::by_name("lexical"),
        Some(WeightProfile::LEXICAL)
    );
    assert_eq!(
        WeightProfile::by_name("graph_first"),
        Some(WeightProfile::GRAPH_FIRST)
    );
    // Unknown names are rejected rather than silently changing behavior.
    assert_eq!(WeightProfile::by_name("nope"), None);

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
    assert_eq!(parsed.retrieval.limits.search_default_limit, 5);
    assert_eq!(parsed.queue.max_attempts, 5);
    assert_eq!(parsed.ingestion.batch_size, 16);
    assert_eq!(
        parsed.scoring.drop_threshold,
        crate::memory::score::DEFAULT_DROP_THRESHOLD
    );
    assert!(parsed.sync_budget.max_tokens_per_sync.is_none());
}

#[test]
fn config_validation_rejects_degenerate_values() {
    let mut config = MemoryConfig::new("/tmp/ws");
    assert!(config.validate().is_ok());

    config.embedding.dim = 0;
    assert!(config
        .validate()
        .unwrap_err()
        .to_string()
        .contains("embedding.dim"));
    config.embedding.dim = 3;
    config.tree.summary_overhead_reserve_tokens = config.tree.input_token_budget;
    assert!(config
        .validate()
        .unwrap_err()
        .to_string()
        .contains("summary_overhead"));
    config.tree.summary_overhead_reserve_tokens = 10;
    config.retrieval.default_profile.graph = -1.0;
    assert!(config
        .validate()
        .unwrap_err()
        .to_string()
        .contains("weights"));
    config.retrieval.default_profile = WeightProfile {
        graph: 0.0,
        vector: 0.0,
        keyword: 0.0,
        freshness: 0.0,
    };
    assert!(config
        .validate()
        .unwrap_err()
        .to_string()
        .contains("at least one"));

    config.retrieval.default_profile = WeightProfile::BALANCED;
    config.queue.retry_cap_ms = config.queue.retry_base_ms - 1;
    assert!(config.validate().unwrap_err().to_string().contains("queue"));
    config.queue = QueueConfig::default();
    config.retrieval.limits.window_chunk_page_size = 0;
    assert!(config
        .validate()
        .unwrap_err()
        .to_string()
        .contains("retrieval"));
}

#[test]
fn from_toml_file_applies_partial_defaults_and_validates() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("memory.toml");
    std::fs::write(
        &path,
        "workspace = '/tmp/ws'\n[embedding]\nmodel = 'custom'\n",
    )
    .unwrap();

    let config = MemoryConfig::from_toml_file(&path).unwrap();

    assert_eq!(config.embedding.model, "custom");
    assert_eq!(config.embedding.dim, DEFAULT_EMBEDDING_DIM);

    std::fs::write(&path, "workspace = '/tmp/ws'\n[embedding]\ndim = 0\n").unwrap();
    assert!(MemoryConfig::from_toml_file(&path).is_err());
}

#[test]
fn composio_validation_rejects_bad_url_and_blank_injected_secret() {
    let mut config = MemoryConfig::new("/tmp/ws");
    config.sync.composio = Some(ComposioSyncConfig {
        mode: ComposioMode::Direct,
        base_url: "backend.example".into(),
        api_key: None,
        bearer_token: None,
        entity_id: None,
    });
    assert!(config
        .validate()
        .unwrap_err()
        .to_string()
        .contains("base_url"));
    config.sync.composio.as_mut().unwrap().base_url = "https://backend.example".into();
    config.sync.composio.as_mut().unwrap().api_key = Some(SecretString::new("  "));
    assert!(config
        .validate()
        .unwrap_err()
        .to_string()
        .contains("api_key"));
}
