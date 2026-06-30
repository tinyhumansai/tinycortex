//! Snapshot-based change tracking for memory sources.
//!
//! After each sync, this module captures what's in the chunk store for a source,
//! then diffs against previous snapshots to surface additions, removals, and
//! modifications — helping agents understand how their world view has changed
//! over time.
//!
//! Snapshots are built from **already-ingested** data (supplied by an injected
//! [`SnapshotItemSource`], not by re-calling source readers), so diffs cost no
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
//! See [`ledger`] for the mapping and [`DiffEngine`] for the operations.
//!
//! ## Decoupling from `chunks`
//!
//! The chunk store is ported separately, so the engine takes a
//! [`source::SnapshotItemSource`] by injection rather than hard-depending on
//! `chunks`. [`source::InMemoryItemSource`] is a reference/test backend.
//!
//! ## Operations
//!
//! - [`DiffEngine::take_snapshot`] / [`DiffEngine::auto_snapshot_after_sync`]
//! - [`DiffEngine::list_snapshots`]
//! - [`DiffEngine::compute_diff`] (explicit pair)
//! - [`DiffEngine::diff_since_last`] (latest vs previous)
//! - [`DiffEngine::diff_since_read`] (latest vs read marker, optional advance)
//! - [`DiffEngine::mark_read`]
//! - [`DiffEngine::create_checkpoint`] / [`DiffEngine::list_checkpoints`]
//! - [`DiffEngine::diff_since_checkpoint`] (cross-source)
//! - [`DiffEngine::cleanup`]

use std::path::PathBuf;

pub mod checkpoint;
pub mod diff;
pub mod ledger;
pub mod snapshot;
pub mod source;
pub mod types;

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
pub struct DiffEngine<S: SnapshotItemSource> {
    workspace: PathBuf,
    items: S,
}

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

#[cfg(test)]
#[path = "engine_tests.rs"]
mod tests;
