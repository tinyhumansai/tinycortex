//! Tests for the tree factory.

use super::*;

#[test]
fn source_factory_uses_source_kind_and_full_scope() {
    let f = TreeFactory::source("slack:#eng");
    assert_eq!(f.kind(), TreeKind::Source);
    assert_eq!(f.scope(), "slack:#eng");
    assert_eq!(f.profile(), TreeProfile::Source);
}

#[test]
fn global_uses_global_scope_and_kind() {
    let g = TreeFactory::global();
    assert_eq!(g.kind(), TreeKind::Global);
    assert_eq!(g.scope(), GLOBAL_SCOPE);
}

#[test]
fn source_label_strategy_extracts_topic_empty() {
    assert!(matches!(
        TreeFactory::source("slack:#eng").label_strategy(),
        LabelStrategy::ExtractFromContent(_)
    ));
    assert!(matches!(
        TreeFactory::topic("email:alice@example.com").label_strategy(),
        LabelStrategy::Empty
    ));
}

#[test]
fn from_tree_round_trips_kind() {
    let tree = Tree {
        id: "t".into(),
        kind: TreeKind::Topic,
        scope: "person:alice".into(),
        root_id: None,
        max_level: 0,
        status: crate::memory::tree::store::TreeStatus::Active,
        created_at: chrono::Utc::now(),
        last_sealed_at: None,
    };
    let f = TreeFactory::from_tree(&tree);
    assert_eq!(f.kind(), TreeKind::Topic);
    assert_eq!(f.scope(), "person:alice");
}
