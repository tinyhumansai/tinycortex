//! Domain types for snapshot-based memory-source change tracking.
//!
//! These mirror OpenHuman's `memory_diff::types` wire contract: the serde
//! `rename_all = "snake_case"` enums and the field names of [`Snapshot`],
//! [`ItemChange`], [`DiffSummary`], [`DiffResult`], [`Checkpoint`], and
//! [`CrossSourceDiff`] are part of the published RPC/tool surface and must be
//! preserved byte-for-byte when ported.

use serde::{Deserialize, Serialize};

/// What caused a snapshot to be taken.
///
/// `auto` snapshots are captured after a successful source sync;
/// `manual` snapshots are captured on explicit request (RPC/tool or as part of
/// checkpoint baselining).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotTrigger {
    /// Captured automatically after a sync completed.
    Auto,
    /// Captured on explicit user/agent request.
    Manual,
}

impl SnapshotTrigger {
    /// The git-trailer / wire string for this trigger.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Manual => "manual",
        }
    }
}

/// A point-in-time capture of one source's ingested items.
///
/// `id` is the git commit SHA of the snapshot commit in the diff ledger. The
/// remaining fields are reconstructed from the commit-message trailers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    /// Commit SHA of the snapshot commit (the snapshot's stable id).
    pub id: String,
    /// Logical source this snapshot belongs to.
    pub source_id: String,
    /// Source kind wire string (e.g. `folder`, `composio`).
    pub source_kind: String,
    /// Human-readable source label at capture time.
    pub label: String,
    /// Why the snapshot was taken.
    pub trigger: SnapshotTrigger,
    /// Number of items materialised into the snapshot.
    pub item_count: u32,
    /// Capture time in milliseconds since the Unix epoch.
    pub taken_at_ms: i64,
}

/// The kind of change an item underwent between two snapshots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    /// Item present in `to` but not `from`.
    Added,
    /// Item present in `from` but not `to`.
    Removed,
    /// Item present in both with differing content.
    Modified,
}

/// A single item-level change between two snapshots.
///
/// Item identity is the item id (file name in the ledger tree), never the
/// title — so an item rename is reported as a [`ChangeKind::Removed`] plus a
/// [`ChangeKind::Added`], not a modification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemChange {
    /// Stable item id (decoded from the ledger path component).
    pub item_id: String,
    /// Display title derived from the item content (or the id as a fallback).
    pub title: String,
    /// What kind of change occurred.
    pub kind: ChangeKind,
    /// Content hash on the `from` side, absent when the item is newly added.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_content_hash: Option<String>,
    /// Content hash on the `to` side, absent when the item was removed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_content_hash: Option<String>,
    /// Optional bounded unified text diff (only for modifications, on request).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_diff: Option<String>,
}

/// Aggregate change counts for a diff.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DiffSummary {
    /// Items added.
    pub added: u32,
    /// Items removed.
    pub removed: u32,
    /// Items modified.
    pub modified: u32,
    /// Items present and unchanged.
    pub unchanged: u32,
}

/// The result of diffing one source between two snapshots.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffResult {
    /// Source this diff covers.
    pub source_id: String,
    /// Source kind wire string.
    pub source_kind: String,
    /// Source label at the `to` snapshot.
    pub source_label: String,
    /// Baseline snapshot id, or `None` for a first-ever diff (all added).
    pub from_snapshot_id: Option<String>,
    /// Target snapshot id.
    pub to_snapshot_id: String,
    /// Aggregate counts.
    pub summary: DiffSummary,
    /// Per-item changes.
    pub changes: Vec<ItemChange>,
}

/// A named, cross-source baseline: the latest snapshot per source at a moment
/// in time, recorded as an annotated git tag.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    /// Tag name (`ckpt_<uuid>`).
    pub id: String,
    /// Human-readable label.
    pub label: String,
    /// Creation time in milliseconds since the Unix epoch.
    pub created_at_ms: i64,
    /// Per-source head snapshot ids captured at checkpoint time.
    pub snapshot_ids: Vec<String>,
}

/// An aggregate diff across all sources since a checkpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossSourceDiff {
    /// Checkpoint the diff is measured from, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkpoint_id: Option<String>,
    /// When the cross-source diff was computed (ms since the Unix epoch).
    pub computed_at_ms: i64,
    /// Summed counts across every changed source.
    pub summary: DiffSummary,
    /// Per-source diffs (sources unchanged since the checkpoint are omitted).
    pub per_source: Vec<DiffResult>,
}

/// One materialised source item: an item id and its concatenated content.
///
/// This is the unit a snapshot stores as a single git blob; the item id becomes
/// the (encoded) blob name and is the unit of identity for diffs. Produced by a
/// [`SnapshotItemSource`](crate::memory::diff::SnapshotItemSource) in the
/// canonical grouped-and-ordered form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotItem {
    /// Stable item id within the source.
    pub item_id: String,
    /// Concatenated item content (all chunk bodies, in sequence).
    pub content: String,
}

/// Lightweight description of a memory source, sufficient for snapshot metadata.
///
/// The diff layer does not own the source registry (that is a separate ported
/// module). Callers pass the id/kind/label they already hold so the diff engine
/// stays decoupled from `sources` and `chunks`.
#[derive(Debug, Clone)]
pub struct SourceDescriptor {
    /// Logical source id.
    pub id: String,
    /// Source kind wire string (e.g. `folder`, `composio`).
    pub kind: String,
    /// Human-readable label.
    pub label: String,
}

impl SourceDescriptor {
    /// Construct a descriptor from its parts.
    pub fn new(id: impl Into<String>, kind: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            kind: kind.into(),
            label: label.into(),
        }
    }
}

#[cfg(test)]
#[path = "types_tests.rs"]
mod tests;
