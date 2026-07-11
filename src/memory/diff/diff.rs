//! Pairwise and read-marker diff operations for the diff engine.

use anyhow::{anyhow, bail, Result};

use super::source::SnapshotItemSource;
use super::types::{DiffResult, SourceDescriptor};
use super::DiffEngine;

impl<S: SnapshotItemSource> DiffEngine<S> {
    /// Compute the diff between two snapshots of the same source.
    ///
    /// `from_snapshot_id` is `None` for a first-ever diff (everything added).
    /// Cross-source diffs are rejected: both snapshots must belong to the same
    /// source.
    pub fn compute_diff(
        &self,
        from_snapshot_id: Option<&str>,
        to_snapshot_id: &str,
        include_text_diff: bool,
    ) -> Result<DiffResult> {
        let ledger = self.ledger()?;
        let to_snap = ledger
            .get_snapshot(to_snapshot_id)?
            .ok_or_else(|| anyhow!("snapshot not found: {to_snapshot_id}"))?;

        let from_snap = match from_snapshot_id {
            Some(fid) => {
                let s = ledger
                    .get_snapshot(fid)?
                    .ok_or_else(|| anyhow!("snapshot not found: {fid}"))?;
                if s.source_id != to_snap.source_id {
                    bail!(
                        "cross-source diff not allowed: from={} to={}",
                        s.source_id,
                        to_snap.source_id
                    );
                }
                Some(s)
            }
            None => None,
        };

        let (changes, summary) = ledger.compute_changes(
            from_snapshot_id,
            &to_snap.id,
            &to_snap.source_id,
            to_snap.item_count,
            include_text_diff,
        )?;

        Ok(DiffResult {
            source_id: to_snap.source_id.clone(),
            source_kind: to_snap.source_kind.clone(),
            source_label: to_snap.label.clone(),
            from_snapshot_id: from_snap.map(|s| s.id),
            to_snapshot_id: to_snap.id.clone(),
            summary,
            changes,
        })
    }

    /// Diff current state (latest snapshot) against the previous snapshot for a
    /// source. With one snapshot the whole source is reported as added; with
    /// none it is an error.
    pub fn diff_since_last(&self, source_id: &str, include_text_diff: bool) -> Result<DiffResult> {
        let snapshots = {
            let ledger = self.ledger()?;
            ledger.latest_snapshots_for_source(source_id, 2)?
        };

        match snapshots.len() {
            0 => Err(anyhow!("no snapshots found for this source")),
            1 => self.compute_diff(None, &snapshots[0].id, include_text_diff),
            _ => self.compute_diff(Some(&snapshots[1].id), &snapshots[0].id, include_text_diff),
        }
    }

    /// Diff a source's latest snapshot against its read marker — i.e. everything
    /// that changed since the agent last *read* this source's diff.
    ///
    /// When `commit` is true, the read marker (a git ref) is advanced to the
    /// head snapshot after the diff is computed, so a subsequent call returns
    /// only newer changes. With `commit = false` it previews without
    /// acknowledging. If the marker points at a commit that no longer resolves,
    /// it is treated as unread (full diff).
    ///
    /// NOTE: the marker is read, the diff computed, and (optionally) the
    /// marker advanced as three separate ledger operations rather than one
    /// atomic read-modify-write. Under concurrent `commit = true` calls for the
    /// same source, a marker can be force-set to an older snapshot after a
    /// newer commit already advanced it, moving the read position backwards
    /// and causing already-read changes to be re-reported as unread.
    pub fn diff_since_read(
        &self,
        source_id: &str,
        include_text_diff: bool,
        commit: bool,
    ) -> Result<DiffResult> {
        let (head, base_id) = {
            let ledger = self.ledger()?;
            let head = ledger
                .latest_snapshots_for_source(source_id, 1)?
                .into_iter()
                .next();
            let marker = ledger.get_read_marker(source_id)?;
            let base_id = match marker {
                Some(snap_id) if ledger.get_snapshot(&snap_id)?.is_some() => Some(snap_id),
                _ => None,
            };
            (head, base_id)
        };

        let head = head.ok_or_else(|| anyhow!("no snapshots found for this source"))?;

        let diff = self.compute_diff(base_id.as_deref(), &head.id, include_text_diff)?;

        if commit {
            let ledger = self.ledger()?;
            ledger.set_read_marker(source_id, &head.id)?;
        }

        Ok(diff)
    }

    /// Commit read markers for one or more sources, advancing each to its
    /// current head snapshot. Sources without any snapshot are skipped. Returns
    /// the number of markers set.
    ///
    /// The caller supplies the source ids explicitly — the diff layer does not
    /// own the source registry. Pass every enabled source's id to mark "all".
    pub fn mark_read(&self, source_ids: &[String]) -> Result<u64> {
        let ledger = self.ledger()?;
        let mut count = 0u64;
        for sid in source_ids {
            if let Some(head) = ledger
                .latest_snapshots_for_source(sid, 1)?
                .into_iter()
                .next()
            {
                ledger.set_read_marker(sid, &head.id)?;
                count += 1;
            }
        }
        Ok(count)
    }
}

/// Convenience overloads accepting a [`SourceDescriptor`] for ergonomics with
/// callers that already hold one.
impl<S: SnapshotItemSource> DiffEngine<S> {
    /// [`diff_since_last`](Self::diff_since_last) keyed by descriptor.
    pub fn diff_since_last_for(
        &self,
        source: &SourceDescriptor,
        include_text_diff: bool,
    ) -> Result<DiffResult> {
        self.diff_since_last(&source.id, include_text_diff)
    }

    /// [`diff_since_read`](Self::diff_since_read) keyed by descriptor.
    pub fn diff_since_read_for(
        &self,
        source: &SourceDescriptor,
        include_text_diff: bool,
        commit: bool,
    ) -> Result<DiffResult> {
        self.diff_since_read(&source.id, include_text_diff, commit)
    }
}
