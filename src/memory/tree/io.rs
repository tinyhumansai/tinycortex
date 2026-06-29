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
    pub chunk_id: String,
    pub token_count: u32,
    pub timestamp: DateTime<Utc>,
    pub content: String,
    #[serde(default)]
    pub entities: Vec<String>,
    #[serde(default)]
    pub topics: Vec<String>,
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
    pub tree_id: String,
    pub tree_kind: TreeKind,
    pub leaf: TreeLeafPayload,
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
    pub tree_id: String,
    /// Starting node. `None` → start from the tree root.
    #[serde(default)]
    pub start_node_id: Option<String>,
    /// Maximum levels to descend. `0` returns an empty result.
    pub max_depth: u32,
    /// Optional natural-language query; when `Some`, hits are reranked by cosine
    /// similarity to the query embedding (hits without an embedding sort last).
    #[serde(default)]
    pub query: Option<String>,
    /// Max hits to return. `None` → backend default.
    #[serde(default)]
    pub limit: Option<usize>,
}

/// One hit returned by a tree read.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TreeReadHit {
    pub node_id: String,
    /// `"summary"` for sealed nodes, `"chunk"` for leaves.
    pub node_kind: String,
    pub level: u32,
    pub content: String,
    /// Cosine similarity when `query` was set; `0.0` otherwise.
    #[serde(default)]
    pub score: f32,
}

/// Result of a tree read.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TreeReadResult {
    pub hits: Vec<TreeReadHit>,
    /// Total matches BEFORE `limit` truncation.
    pub total: usize,
    pub tree_id: String,
}

impl TreeReadResult {
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
