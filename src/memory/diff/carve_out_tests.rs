//! The `git-diff`-disabled half of the diff module's contract.
//!
//! Sibling test file rather than an inline `mod`, matching this directory's
//! convention (`ledger_tests.rs`, `source_tests.rs`, `types_tests.rs`, …).

use super::types::{ChangeKind, CrossSourceDiff, DiffSummary, SnapshotItem};
use super::SnapshotItemSource;

#[test]
fn inert_diff_types_are_available_without_the_git_diff_feature() {
    // Round-tripped, not just serialised: these types exist to cross a
    // boundary, so a `Deserialize` derive that got gated away has to fail here
    // too. Serialising alone would only exercise half the pair.
    let diff = CrossSourceDiff {
        checkpoint_id: Some("ckpt_1".into()),
        computed_at_ms: 0,
        summary: DiffSummary::default(),
        per_source: Vec::new(),
    };
    let json = serde_json::to_string(&diff).expect("CrossSourceDiff serialises");
    let restored: CrossSourceDiff =
        serde_json::from_str(&json).expect("CrossSourceDiff deserialises");
    assert_eq!(restored.checkpoint_id.as_deref(), Some("ckpt_1"));
    assert_eq!(restored.computed_at_ms, 0);
    assert!(restored.per_source.is_empty());
    let _kind = ChangeKind::Added;
}

#[test]
fn the_item_source_trait_can_still_be_implemented_without_git() {
    struct Empty;
    impl SnapshotItemSource for Empty {
        fn items_for_source(&self, _source_id: &str) -> Vec<SnapshotItem> {
            Vec::new()
        }
    }
    assert!(Empty.items_for_source("anything").is_empty());
}
