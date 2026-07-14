//! Tests for the generic tree registry.

use super::*;
use tempfile::TempDir;

fn test_config() -> (TempDir, MemoryConfig) {
    let tmp = TempDir::new().unwrap();
    let cfg = MemoryConfig::new(tmp.path());
    (tmp, cfg)
}

#[test]
fn get_or_create_is_idempotent_on_scope() {
    let (_tmp, cfg) = test_config();
    let first = get_or_create_tree(&cfg, TreeKind::Source, "slack:#eng").unwrap();
    let second = get_or_create_tree(&cfg, TreeKind::Source, "slack:#eng").unwrap();
    assert_eq!(first.id, second.id);
    assert_eq!(first.kind, TreeKind::Source);
    assert_eq!(first.status, TreeStatus::Active);
}

#[test]
fn different_scopes_yield_different_trees() {
    let (_tmp, cfg) = test_config();
    let a = get_or_create_tree(&cfg, TreeKind::Source, "slack:#eng").unwrap();
    let b = get_or_create_tree(&cfg, TreeKind::Source, "gmail:user@example.com").unwrap();
    assert_ne!(a.id, b.id);
    assert_ne!(a.scope, b.scope);
}

#[test]
fn different_kinds_same_scope_yield_different_trees() {
    let (_tmp, cfg) = test_config();
    let source = get_or_create_tree(&cfg, TreeKind::Source, "shared:scope").unwrap();
    let topic = get_or_create_tree(&cfg, TreeKind::Topic, "shared:scope").unwrap();
    assert_ne!(source.id, topic.id);
    assert_eq!(source.kind, TreeKind::Source);
    assert_eq!(topic.kind, TreeKind::Topic);
}

#[test]
fn tree_id_has_expected_prefix() {
    assert!(new_tree_id(TreeKind::Source).starts_with("source:"));
    assert!(new_tree_id(TreeKind::Topic).starts_with("topic:"));
    assert!(new_tree_id(TreeKind::Global).starts_with("global:"));

    let sum_id = new_summary_id(3);
    assert!(sum_id.starts_with("summary:"));
    assert!(sum_id.contains(":L3-"), "expected level suffix in {sum_id}");
}

#[test]
fn summary_id_format_is_lexicographically_chronological() {
    let earlier = format!("summary:{:013}:L1-{:08x}", 1_700_000_000_000u64, u32::MAX);
    let later = format!("summary:{:013}:L9-{:08x}", 1_700_000_000_001u64, 0u32);
    assert!(earlier < later, "ms must outrank level + tail");

    let live = new_summary_id(2);
    let rest = &live["summary:".len()..];
    let ms_part = rest.split(':').next().unwrap();
    assert_eq!(ms_part.len(), 13);
    assert!(ms_part.chars().all(|c| c.is_ascii_digit()));
}

#[test]
fn get_or_create_recovers_from_unique_race() {
    let (_tmp, cfg) = test_config();
    let pre_existing = Tree {
        id: "source:preexisting".into(),
        kind: TreeKind::Source,
        scope: "slack:#eng".into(),
        root_id: None,
        max_level: 0,
        status: TreeStatus::Active,
        created_at: Utc::now(),
        last_sealed_at: None,
        ask: None,
    };
    store::insert_tree(&cfg, &pre_existing).unwrap();
    let got = get_or_create_tree(&cfg, TreeKind::Source, "slack:#eng").unwrap();
    assert_eq!(got.id, "source:preexisting");

    let dup = Tree {
        id: "source:would-collide".into(),
        ..pre_existing.clone()
    };
    let err = store::insert_tree(&cfg, &dup).unwrap_err();
    assert!(
        is_unique_violation(&err),
        "expected UNIQUE violation: {err:#}"
    );
}
