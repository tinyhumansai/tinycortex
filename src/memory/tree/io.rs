//! Canonical input/output contract types for the tree module.
//!
//! Two fundamental operations against any tree:
//! - **Write** — append a chunk (leaf); cascading seals may produce summaries.
//! - **Read**  — navigate from a node into its descendants.
//!
//! These are pure contract types — no logic, no IO. They compose the engine
//! primitives ([`LeafRef`], the store rows) into one serde-friendly shape per
//! direction so callers above the tree module talk to it uniformly.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::memory::tree::bucket_seal::{LabelStrategy, LeafRef};
use crate::memory::tree::store::{Tree, TreeKind};

// ───────────────────────── Write ─────────────────────────

/// A leaf payload ready to be appended to a tree. Serde-friendly mirror of
/// [`LeafRef`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TreeLeafPayload {
    /// Stable chunk id; ties the leaf back to its chunk store row.
    pub chunk_id: String,
    /// Token count of [`content`](Self::content); drives buffer/seal sizing.
    pub token_count: u32,
    /// Source timestamp of the underlying chunk (UTC).
    pub timestamp: DateTime<Utc>,
    /// Raw leaf text.
    pub content: String,
    /// Entity ids attached to the leaf; propagated to summaries under
    /// [`TreeLabelStrategy::Inherit`].
    #[serde(default)]
    pub entities: Vec<String>,
    /// Topic labels attached to the leaf; propagated like `entities`.
    #[serde(default)]
    pub topics: Vec<String>,
    /// Importance/relevance score in `[0.0, 1.0]`; defaults to `0.0`.
    #[serde(default)]
    pub score: f32,
}

impl From<&TreeLeafPayload> for LeafRef {
    fn from(p: &TreeLeafPayload) -> Self {
        LeafRef {
            chunk_id: p.chunk_id.clone(),
            token_count: p.token_count,
            timestamp: p.timestamp,
            content: p.content.clone(),
            entities: p.entities.clone(),
            topics: p.topics.clone(),
            score: p.score,
        }
    }
}

impl From<LeafRef> for TreeLeafPayload {
    fn from(l: LeafRef) -> Self {
        Self {
            chunk_id: l.chunk_id,
            token_count: l.token_count,
            timestamp: l.timestamp,
            content: l.content,
            entities: l.entities,
            topics: l.topics,
            score: l.score,
        }
    }
}

/// How sealed summaries should be labelled with entities/topics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TreeLabelStrategy {
    /// Inherit entities/topics from child leaves (default).
    #[default]
    Inherit,
    /// Re-extract from the summary text.
    Extract,
    /// Leave entities/topics empty.
    Empty,
}

impl TreeLabelStrategy {
    /// Resolve into a runtime [`LabelStrategy`]. `Extract` requires an extractor
    /// supplied by the caller; when `None`, it degrades to `Inherit`.
    pub fn resolve(
        self,
        extractor: Option<std::sync::Arc<dyn crate::memory::score::extract::EntityExtractor>>,
    ) -> LabelStrategy {
        match self {
            TreeLabelStrategy::Inherit => LabelStrategy::UnionFromChildren,
            TreeLabelStrategy::Empty => LabelStrategy::Empty,
            TreeLabelStrategy::Extract => match extractor {
                Some(ex) => LabelStrategy::ExtractFromContent(ex),
                None => LabelStrategy::UnionFromChildren,
            },
        }
    }
}

/// Canonical write request: "append this leaf to this tree".
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TreeWriteRequest {
    /// Target tree id.
    pub tree_id: String,
    /// Discriminates the tree's domain/shape (see [`TreeKind`]).
    pub tree_kind: TreeKind,
    /// Leaf to append.
    pub leaf: TreeLeafPayload,
    /// How summaries sealed by this append are labelled; defaults to
    /// [`TreeLabelStrategy::Inherit`].
    #[serde(default)]
    pub label_strategy: TreeLabelStrategy,
    /// When `true`, only stage the leaf in the L0 buffer; do not cascade seals
    /// synchronously.
    #[serde(default)]
    pub deferred: bool,
}

/// Canonical write outcome.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TreeWriteOutcome {
    /// Ids of summary nodes that sealed during this append.
    pub new_summary_ids: Vec<String>,
    /// Set when the caller used `deferred = true` and should enqueue a seal job.
    pub seal_pending: bool,
}

// ───────────────────────── Read ─────────────────────────

/// What the caller wants out of a read. Bounds the traversal and controls
/// query-driven reranking.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TreeReadRequest {
    /// Id of the tree to read from.
    pub tree_id: String,
    /// Starting node. `None` → start from the tree root.
    #[serde(default)]
    pub start_node_id: Option<String>,
    /// Maximum levels to descend. `0` returns an empty result.
    pub max_depth: u32,
    /// Optional natural-language query. When `Some`, [`crate::memory::tree::read::read_tree`]
    /// scores each hit by a cheap lowercase token-overlap heuristic (count of
    /// query terms found as substrings of the hit content) and sorts
    /// descending — **not** cosine similarity against an embedding. Full
    /// embedding-based hybrid retrieval lives in the separate `retrieval`
    /// module; see `TR-13` in `docs/spec/audit/03-tree-archivist-conversations.md`.
    #[serde(default)]
    pub query: Option<String>,
    /// Max hits to return. `None` → backend default.
    #[serde(default)]
    pub limit: Option<usize>,
}

/// One hit returned by a tree read.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TreeReadHit {
    /// Id of the matched node (summary node id or leaf chunk id).
    pub node_id: String,
    /// `"summary"` for sealed nodes, `"chunk"` for leaves.
    pub node_kind: String,
    /// Tree level of the node; `0` is the leaf layer, higher levels are summaries.
    pub level: u32,
    /// Node text (summary or leaf content).
    pub content: String,
    /// Token-overlap match count against [`TreeReadRequest::query`] when a query
    /// was set (not a normalized similarity — can exceed `1.0`); `0.0` when no
    /// query was supplied.
    #[serde(default)]
    pub score: f32,
}

/// Result of a tree read.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TreeReadResult {
    /// Returned hits, already truncated to the request `limit`.
    pub hits: Vec<TreeReadHit>,
    /// Total matches BEFORE `limit` truncation.
    pub total: usize,
    /// Tree the read ran against.
    pub tree_id: String,
}

impl TreeReadResult {
    /// Empty result carrying `tree`'s id; used when a read matches nothing.
    pub fn empty(tree: &Tree) -> Self {
        Self {
            hits: Vec::new(),
            total: 0,
            tree_id: tree.id.clone(),
        }
    }
}

#[cfg(test)]
#[path = "io_tests.rs"]
mod tests;
