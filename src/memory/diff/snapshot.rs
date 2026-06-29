//! Snapshot capture for the diff engine.
//!
//! Snapshots are built from already-ingested data supplied by the injected
//! [`SnapshotItemSource`](super::source::SnapshotItemSource) — never by
//! re-calling source readers — and committed to the git ledger. The chunk
//! store (whatever backs the item source) remains authoritative; the ledger is
//! a derived, rebuildable view used purely for change tracking.

use anyhow::Result;

use super::ledger::{Ledger, SnapshotMeta};
use super::source::SnapshotItemSource;
use super::types::{Snapshot, SnapshotTrigger, SourceDescriptor};
use super::DiffEngine;

impl<S: SnapshotItemSource> DiffEngine<S> {
    /// Take a snapshot of the current item-source state for a source.
    ///
    /// Reads the source's already-ingested items (grouped by item id and
    /// ordered), then commits one blob per item to the git ledger. Returns the
    /// new [`Snapshot`] whose `id` is the commit SHA.
    pub fn take_snapshot(
        &self,
        source: &SourceDescriptor,
        trigger: SnapshotTrigger,
    ) -> Result<Snapshot> {
        let items: Vec<(String, String)> = self
            .items
            .items_for_source(&source.id)
            .into_iter()
            .map(|item| (item.item_id, item.content))
            .collect();

        let meta = SnapshotMeta {
            source_id: source.id.clone(),
            source_kind: source.kind.clone(),
            label: source.label.clone(),
            trigger,
        };
        let now_ms = self.now_ms();

        let ledger = self.ledger()?;
        ledger.commit_snapshot(&meta, &items, now_ms)
    }

    /// Auto-snapshot hook to call after a successful source sync. Equivalent to
    /// [`take_snapshot`](Self::take_snapshot) with [`SnapshotTrigger::Auto`].
    pub fn auto_snapshot_after_sync(&self, source: &SourceDescriptor) -> Result<Snapshot> {
        self.take_snapshot(source, SnapshotTrigger::Auto)
    }

    /// List snapshots newest-first, optionally filtered to one source.
    pub fn list_snapshots(&self, source_id: Option<&str>, limit: u32) -> Result<Vec<Snapshot>> {
        let ledger = self.ledger()?;
        ledger.list_snapshots(source_id, limit)
    }

    /// Open a ledger handle rooted at this engine's workspace.
    pub(super) fn ledger(&self) -> Result<Ledger> {
        Ledger::open(&self.workspace)
    }
}
