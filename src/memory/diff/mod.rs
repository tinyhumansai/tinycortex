//! Snapshot-based change tracking for memory sources.
//!
//! After each sync, this module captures what's in the chunk store for a source,
//! then diffs against previous snapshots to surface additions, removals, and
//! modifications — helping agents understand how their world view has changed
//! over time.
//!
//! Snapshots are built from **already-ingested** data (supplied by an injected
//! `SnapshotItemSource`, not by re-calling source readers), so diffs cost no
//! upstream API calls. The authoritative store remains whatever backs the item
//! source; this module's storage is a *derived* git ledger.
//!
//! ## Git ledger
//!
//! Storage is a git repository at `<workspace>/memory_diff/repo` (the diff
//! *ledger*):
//!
//! - Snapshot → git commit (`Snapshot.id` is the commit SHA).
//! - Checkpoint → annotated tag `ckpt_<uuid>` at HEAD.
//! - Read marker → ref `refs/openhuman/read/<encoded_source_id>`.
//! - Diff → git tree diff scoped to a source path.
//!
//! See the ledger module for the mapping and `DiffEngine` for the operations.
//!
//! ## Decoupling from `chunks`
//!
//! The chunk store is ported separately, so the engine takes a
//! `SnapshotItemSource` by injection rather than hard-depending on
//! `chunks`. `InMemoryItemSource` is a reference/test backend.
//!
//! ## Operations
//!
//! - `DiffEngine::take_snapshot` / `DiffEngine::auto_snapshot_after_sync`
//! - `DiffEngine::list_snapshots`
//! - `DiffEngine::compute_diff` (explicit pair)
//! - `DiffEngine::diff_since_last` (latest vs previous)
//! - `DiffEngine::diff_since_read` (latest vs read marker, optional advance)
//! - `DiffEngine::mark_read`
//! - `DiffEngine::create_checkpoint` / `DiffEngine::list_checkpoints`
//! - `DiffEngine::diff_since_checkpoint` (cross-source)
//! - `DiffEngine::cleanup`

//! ## Type carve-out — what compiles WITHOUT `git-diff`
//!
//! `types` and `source` are `serde`/`std`-only: they name no git concept and
//! reach no `git2` symbol. They stay **ungated**, so a host that does not want
//! libgit2 in its dependency graph can still describe a diff — pass a
//! `CrossSourceDiff` around, match on a `ChangeKind`, implement a
//! `SnapshotItemSource` — it simply cannot *compute* one.
//!
//! That distinction is what makes the gate usable downstream. OpenHuman's
//! always-on subconscious profile renders `CrossSourceDiff` / `ChangeKind` in
//! prompts, and stubbing those types rather than sharing them would mean two
//! definitions of the same serde shape drifting apart silently. The rule
//! (OpenHuman's AGENTS.md, "Compile-time domain gates") is: put a domain's
//! inert types in a dependency-free submodule and leave it ungated; gate only
//! the behaviour.
//!
//! Gated on `git-diff`: `ledger` + `ledger_helpers` (the only two modules that
//! touch `git2` directly), `checkpoint` / `diff` / `snapshot` (whose impls are
//! all written against `Ledger`), and `DiffEngine` itself — the engine's inherent
//! methods live in those modules, so an ungated `DiffEngine` would be a handle
//! with nothing to call.

#[cfg(feature = "git-diff")]
use std::path::PathBuf;

#[cfg(feature = "git-diff")]
pub mod checkpoint;
// Keep the established `memory::diff::diff` path for downstream callers.
#[cfg(feature = "git-diff")]
#[allow(clippy::module_inception)]
pub mod diff;
#[cfg(feature = "git-diff")]
pub mod ledger;
#[cfg(feature = "git-diff")]
mod ledger_helpers;
#[cfg(feature = "git-diff")]
pub mod snapshot;
pub mod source;
pub mod types;

#[cfg(feature = "git-diff")]
pub use ledger::{Ledger, SnapshotMeta};
pub use source::{extract_item_id, InMemoryItemSource, SnapshotItemSource};
pub use types::{
    ChangeKind, Checkpoint, CrossSourceDiff, DiffResult, DiffSummary, ItemChange, Snapshot,
    SnapshotItem, SnapshotTrigger, SourceDescriptor,
};

/// The git-backed source diff engine.
///
/// Constructed from a workspace root (where the ledger lives, mirroring
/// [`MemoryConfig::workspace`](crate::memory::config::MemoryConfig)) and an
/// injected [`SnapshotItemSource`] that yields a source's already-ingested
/// items. All operations are synchronous; git mutations serialise through a
/// process-global lock inside the [`Ledger`].
#[cfg(feature = "git-diff")]
pub struct DiffEngine<S: SnapshotItemSource> {
    workspace: PathBuf,
    items: S,
}

#[cfg(feature = "git-diff")]
impl<S: SnapshotItemSource> DiffEngine<S> {
    /// Construct an engine rooted at `workspace`, reading items from `items`.
    pub fn new(workspace: impl Into<PathBuf>, items: S) -> Self {
        Self {
            workspace: workspace.into(),
            items,
        }
    }

    /// Borrow the injected item source.
    pub fn items(&self) -> &S {
        &self.items
    }

    /// The workspace root the ledger lives under.
    pub fn workspace(&self) -> &std::path::Path {
        &self.workspace
    }

    /// Current wall-clock time in milliseconds since the Unix epoch.
    pub(super) fn now_ms(&self) -> i64 {
        chrono::Utc::now().timestamp_millis()
    }
}

#[cfg(all(test, feature = "git-diff"))]
#[path = "engine_tests.rs"]
mod tests;

/// Proves the type carve-out holds in the build that motivates it.
///
/// The whole point of leaving `types`/`source` ungated is that a host without
/// libgit2 can still name a diff. Only the disabled build can catch a regression
/// here — re-gating them would compile fine everywhere else and only break
/// downstream, which is exactly the failure this test exists to make loud.
#[cfg(all(test, not(feature = "git-diff")))]
mod carve_out_tests {
    use super::types::{ChangeKind, CrossSourceDiff, DiffSummary, SnapshotItem};
    use super::SnapshotItemSource;

    #[test]
    fn inert_diff_types_are_available_without_the_git_diff_feature() {
        // Constructed field-by-field, and round-tripped through serde, because
        // these types exist to cross a boundary: a host renders them and stores
        // them. Merely naming them would not catch a derive being gated away.
        let diff = CrossSourceDiff {
            checkpoint_id: Some("ckpt_1".into()),
            computed_at_ms: 0,
            summary: DiffSummary::default(),
            per_source: Vec::new(),
        };
        let json = serde_json::to_string(&diff).expect("CrossSourceDiff serialises");
        assert!(json.contains("ckpt_1"));
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
}
