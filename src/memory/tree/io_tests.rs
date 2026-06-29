//! Tests for the tree contract types.

use super::*;
use crate::memory::tree::store::TreeStatus;

fn sample_tree() -> Tree {
    Tree {
        id: "tree-1".into(),
        kind: TreeKind::Source,
        scope: "chat:slack:#eng".into(),
        root_id: Some("root-1".into()),
        max_level: 2,
        status: TreeStatus::Active,
        created_at: Utc::now(),
        last_sealed_at: None,
    }
}

#[test]
fn tree_leaf_payload_converts_to_and_from_leaf_ref() {
    let payload = TreeLeafPayload {
        chunk_id: "chunk-1".into(),
        token_count: 12,
        timestamp: Utc::now(),
        content: "hello".into(),
        entities: vec!["person:alice".into()],
        topics: vec!["deploy".into()],
        score: 0.75,
    };
    let leaf: LeafRef = (&payload).into();
    let roundtrip = TreeLeafPayload::from(leaf);
    assert_eq!(roundtrip.chunk_id, payload.chunk_id);
    assert_eq!(roundtrip.token_count, payload.token_count);
    assert_eq!(roundtrip.content, payload.content);
    assert_eq!(roundtrip.entities, payload.entities);
    assert_eq!(roundtrip.topics, payload.topics);
    assert_eq!(roundtrip.score, payload.score);
}

#[test]
fn tree_read_result_empty_copies_tree_id() {
    let tree = sample_tree();
    let result = TreeReadResult::empty(&tree);
    assert_eq!(result.tree_id, tree.id);
    assert_eq!(result.total, 0);
    assert!(result.hits.is_empty());
}

#[test]
fn label_strategy_default_is_inherit() {
    assert_eq!(TreeLabelStrategy::default(), TreeLabelStrategy::Inherit);
    assert!(matches!(
        TreeLabelStrategy::Inherit.resolve(None),
        LabelStrategy::UnionFromChildren
    ));
    assert!(matches!(
        TreeLabelStrategy::Empty.resolve(None),
        LabelStrategy::Empty
    ));
    // Extract with no extractor degrades to inherit.
    assert!(matches!(
        TreeLabelStrategy::Extract.resolve(None),
        LabelStrategy::UnionFromChildren
    ));
}
