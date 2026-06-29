//! Checkpoint creation, listing, cross-source diff, and cleanup.

use anyhow::{anyhow, Result};

use super::source::SnapshotItemSource;
use super::types::{
    Checkpoint, CrossSourceDiff, DiffResult, DiffSummary, SnapshotTrigger, SourceDescriptor,
};
use super::DiffEngine;

impl<S: SnapshotItemSource> DiffEngine<S> {
    /// Create a checkpoint (git tag at HEAD) grouping the latest snapshot per
    /// supplied source.
    ///
    /// Any source lacking a snapshot is given one (with
    /// [`SnapshotTrigger::Manual`]) so the checkpoint has a baseline for every
    /// source. The caller passes the sources to baseline — the diff layer does
    /// not own the source registry.
    pub fn create_checkpoint(
        &self,
        label: &str,
        sources: &[SourceDescriptor],
    ) -> Result<Checkpoint> {
        // Snapshot any source that doesn't have one yet.
        {
            let ledger = self.ledger()?;
            let lacking: Vec<&SourceDescriptor> = sources
                .iter()
                .filter_map(|s| match ledger.snapshot_count_for_source(&s.id) {
                    Ok(0) => Some(Ok(s)),
                    Ok(_) => None,
                    Err(e) => Some(Err(e)),
                })
                .collect::<Result<Vec<_>>>()?;
            // Drop the ledger handle before taking snapshots (each opens its own).
            drop(ledger);
            for source in lacking {
                self.take_snapshot(source, SnapshotTrigger::Manual)?;
            }
        }

        let checkpoint_id = format!("ckpt_{}", uuid::Uuid::new_v4());
        let created_at_ms = self.now_ms();

        let ledger = self.ledger()?;
        let mut snapshot_ids = Vec::new();
        for source in sources {
            if let Some(snap) = ledger
                .latest_snapshots_for_source(&source.id, 1)?
                .into_iter()
                .next()
            {
                snapshot_ids.push(snap.id);
            }
        }
        ledger.create_checkpoint(&checkpoint_id, label, &snapshot_ids, created_at_ms)?;

        Ok(Checkpoint {
            id: checkpoint_id,
            label: label.to_string(),
            created_at_ms,
            snapshot_ids,
        })
    }

    /// List checkpoints newest-first, up to `limit`.
    pub fn list_checkpoints(&self, limit: u32) -> Result<Vec<Checkpoint>> {
        let ledger = self.ledger()?;
        ledger.list_checkpoints(limit)
    }

    /// Compute a cross-source diff: everything that changed since a checkpoint.
    ///
    /// For each baseline snapshot in the checkpoint, the source's current head
    /// is diffed against it. Sources unchanged since the checkpoint are omitted.
    pub fn diff_since_checkpoint(
        &self,
        checkpoint_id: &str,
        include_text_diff: bool,
    ) -> Result<CrossSourceDiff> {
        let ledger = self.ledger()?;
        let checkpoint = ledger
            .get_checkpoint(checkpoint_id)?
            .ok_or_else(|| anyhow!("checkpoint not found: {checkpoint_id}"))?;

        let computed_at_ms = self.now_ms();
        let mut per_source = Vec::new();
        let mut agg = DiffSummary::default();

        for snap_id in &checkpoint.snapshot_ids {
            let Some(base) = ledger.get_snapshot(snap_id)? else {
                continue;
            };
            let Some(head) = ledger
                .latest_snapshots_for_source(&base.source_id, 1)?
                .into_iter()
                .next()
            else {
                continue;
            };
            if head.id == base.id {
                continue; // unchanged since the checkpoint
            }

            let (changes, summary) = ledger.compute_changes(
                Some(&base.id),
                &head.id,
                &head.source_id,
                head.item_count,
                include_text_diff,
            )?;
            agg.added += summary.added;
            agg.removed += summary.removed;
            agg.modified += summary.modified;
            agg.unchanged += summary.unchanged;
            per_source.push(DiffResult {
                source_id: head.source_id.clone(),
                source_kind: head.source_kind.clone(),
                source_label: head.label.clone(),
                from_snapshot_id: Some(base.id.clone()),
                to_snapshot_id: head.id.clone(),
                summary,
                changes,
            });
        }

        Ok(CrossSourceDiff {
            checkpoint_id: Some(checkpoint.id),
            computed_at_ms,
            summary: agg,
            per_source,
        })
    }

    /// Delete checkpoint tags older than `older_than_days`.
    ///
    /// Snapshot commits are retained — git history *is* the ledger — so cleanup
    /// only prunes named baselines. Returns the number of checkpoints deleted.
    pub fn cleanup(&self, older_than_days: u32) -> Result<u64> {
        let cutoff = self.now_ms() - (older_than_days as i64 * 24 * 60 * 60 * 1000);
        let ledger = self.ledger()?;
        ledger.cleanup_checkpoints(cutoff)
    }
}
