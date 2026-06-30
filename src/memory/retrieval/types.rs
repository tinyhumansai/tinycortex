//! Shared wire types for the retrieval primitives.
//!
//! These mirror OpenHuman's `memory_tree::retrieval::types`: every primitive
//! (`query_source`, reconstructed `query_global` / `query_topic`,
//! `drill_down`, `cover_window`, `fetch_leaves`) emits the same unified
//! [`RetrievalHit`] shape so a caller sees one schema regardless of which
//! primitive ran.
//!
//! Rules of the road:
//! - All types round-trip through JSON ([`serde::Serialize`] +
//!   [`serde::Deserialize`]).
//! - Time fields are `DateTime<Utc>` (serialised RFC3339).
//! - [`NodeKind`] discriminates a raw leaf chunk from a summary node so
//!   consumers can branch (e.g. "drill down only on summaries").

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::memory::chunks::{Chunk, SourceKind};
use crate::memory::score::extract::EntityKind;
use crate::memory::tree::{SummaryNode, Tree, TreeKind};

/// Whether a hit represents a leaf (raw chunk) or a summary node.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    /// Leaf = one `mem_tree_chunks` row (level 0).
    Leaf,
    /// Summary = one `mem_tree_summaries` row (level ≥ 1).
    Summary,
}

impl NodeKind {
    /// Stable lowercase string form (`"leaf"` / `"summary"`) — matches the
    /// serde representation and is suitable for SQL discriminator columns.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Leaf => "leaf",
            Self::Summary => "summary",
        }
    }
}

/// One unit of retrieval output. Shape is identical whether the hit came from
/// a source-tree summary, the entity index, or a raw leaf chunk.
///
/// `tree_id` / `tree_kind` / `tree_scope` identify the provenance tree so a UI
/// can surface "from slack:#eng". For bare leaves not attached to any tree,
/// `tree_id` is empty and `tree_kind` falls back to [`TreeKind::Source`] (see
/// [`leaf_tree_placeholder`]).
///
/// `child_ids` is empty on leaves; on summaries it points one level down
/// (chunks for L1, summaries for L2+).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RetrievalHit {
    pub node_id: String,
    pub node_kind: NodeKind,
    pub tree_id: String,
    pub tree_kind: TreeKind,
    pub tree_scope: String,
    pub level: u32,
    pub content: String,
    pub entities: Vec<String>,
    pub topics: Vec<String>,
    pub time_range_start: DateTime<Utc>,
    pub time_range_end: DateTime<Utc>,
    pub score: f32,
    pub child_ids: Vec<String>,
    /// Populated for leaves (chunk back-pointer); `None` for summaries.
    pub source_ref: Option<String>,
}

/// Envelope for the query primitives (`query_source`, reconstructed
/// `query_global` / `query_topic`, `cover_window`).
///
/// `total` is the pre-truncation match count so callers can tell whether a
/// higher-limit follow-up would return more. `truncated` is
/// `total > hits.len()`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QueryResponse {
    pub hits: Vec<RetrievalHit>,
    pub total: usize,
    pub truncated: bool,
}

impl QueryResponse {
    /// Build a response from a post-filtered, post-sorted hit list. The
    /// `total_matches` count is taken BEFORE applying `limit` so callers can
    /// detect truncation.
    pub fn new(hits: Vec<RetrievalHit>, total_matches: usize) -> Self {
        let truncated = total_matches > hits.len();
        Self {
            hits,
            total: total_matches,
            truncated,
        }
    }

    /// Empty response (no matches). `total = 0`, `truncated = false`.
    pub fn empty() -> Self {
        Self {
            hits: Vec::new(),
            total: 0,
            truncated: false,
        }
    }
}

/// Convert a sealed [`SummaryNode`] into a [`RetrievalHit`]. `tree_scope` is
/// threaded in from the caller so we don't force a tree lookup on every
/// conversion — the caller already has the parent [`Tree`] in hand.
pub fn hit_from_summary(node: &SummaryNode, tree_scope: &str) -> RetrievalHit {
    RetrievalHit {
        node_id: node.id.clone(),
        node_kind: NodeKind::Summary,
        tree_id: node.tree_id.clone(),
        tree_kind: node.tree_kind,
        tree_scope: tree_scope.to_string(),
        level: node.level,
        content: node.content.clone(),
        entities: node.entities.clone(),
        topics: node.topics.clone(),
        time_range_start: node.time_range_start,
        time_range_end: node.time_range_end,
        score: node.score,
        child_ids: node.child_ids.clone(),
        source_ref: None,
    }
}

/// Convert a sealed [`SummaryNode`] using a full [`Tree`] for the scope. A thin
/// convenience over [`hit_from_summary`].
pub fn hit_from_summary_with_tree(node: &SummaryNode, tree: &Tree) -> RetrievalHit {
    hit_from_summary(node, &tree.scope)
}

/// Convert a raw [`Chunk`] (leaf) into a [`RetrievalHit`]. Because a chunk may
/// not yet be attached to a summary tree, callers can pass `tree_id` /
/// `tree_scope` as empty strings. `tree_kind` is always [`TreeKind::Source`]
/// for leaves — raw chunks belong conceptually to their originating source tree
/// even when the tree hasn't materialised yet (no seals).
pub fn hit_from_chunk(chunk: &Chunk, tree_id: &str, tree_scope: &str, score: f32) -> RetrievalHit {
    let source_ref = chunk.metadata.source_ref.as_ref().map(|r| r.value.clone());
    RetrievalHit {
        node_id: chunk.id.clone(),
        node_kind: NodeKind::Leaf,
        tree_id: tree_id.to_string(),
        tree_kind: leaf_tree_placeholder(chunk.metadata.source_kind),
        tree_scope: tree_scope.to_string(),
        level: 0,
        content: chunk.content.clone(),
        entities: Vec::new(),
        topics: chunk.metadata.tags.clone(),
        time_range_start: chunk.metadata.time_range.0,
        time_range_end: chunk.metadata.time_range.1,
        score,
        child_ids: Vec::new(),
        source_ref,
    }
}

/// Decide the placeholder [`TreeKind`] to report on a leaf hit. Leaves live
/// under source trees regardless of the underlying [`SourceKind`], so we always
/// return [`TreeKind::Source`]. Accepting the `SourceKind` argument keeps the
/// call site explicit about why the classification is stable.
pub fn leaf_tree_placeholder(_source_kind: SourceKind) -> TreeKind {
    TreeKind::Source
}

/// Output shape for `search_entities`. One row per canonical id with aggregate
/// stats across the entity index.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EntityMatch {
    /// Canonical id (e.g. `email:alice@example.com`, `topic:phoenix`).
    pub canonical_id: String,
    pub kind: EntityKind,
    /// Example surface form that matched — useful for UI display.
    pub surface: String,
    /// Total rows in `mem_tree_entity_index` grouped under this canonical id.
    pub mention_count: u64,
    /// Epoch-millis of the newest mention across all rows.
    pub last_seen_ms: i64,
}

#[cfg(test)]
#[path = "types_tests.rs"]
mod tests;
