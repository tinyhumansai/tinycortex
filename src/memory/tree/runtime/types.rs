//! Domain types for the markdown time-based summary tree.
//!
//! Organises summaries as a time hierarchy: root → year → month → day → hour
//! (leaf). Ported from OpenHuman's `memory_tree/tree_runtime/types.rs`.

use chrono::{DateTime, Datelike, Timelike, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Hierarchical level of a tree node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeLevel {
    /// Single tree root; aggregates all years. Wire string `"root"`.
    Root,
    /// One node per calendar year. Wire string `"year"`.
    Year,
    /// One node per calendar month. Wire string `"month"`.
    Month,
    /// One node per calendar day. Wire string `"day"`.
    Day,
    /// Leaf level; one node per hour, where raw content lands. Wire string `"hour"`.
    Hour,
}

impl NodeLevel {
    /// Maximum number of tokens allowed at this level.
    pub fn max_tokens(&self) -> u32 {
        match self {
            Self::Hour => 1_000,
            Self::Day => 2_000,
            Self::Month => 4_000,
            Self::Year => 8_000,
            Self::Root => 20_000,
        }
    }

    /// The level above this one in the hierarchy (`None` for root).
    pub fn parent_level(&self) -> Option<NodeLevel> {
        match self {
            Self::Hour => Some(Self::Day),
            Self::Day => Some(Self::Month),
            Self::Month => Some(Self::Year),
            Self::Year => Some(Self::Root),
            Self::Root => None,
        }
    }

    /// True only for the leaf level (hour).
    pub fn is_leaf(&self) -> bool {
        matches!(self, Self::Hour)
    }

    /// Parse a level string from YAML frontmatter.
    pub fn from_str_label(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "root" => Some(Self::Root),
            "year" => Some(Self::Year),
            "month" => Some(Self::Month),
            "day" => Some(Self::Day),
            "hour" => Some(Self::Hour),
            _ => None,
        }
    }

    /// Label for display / frontmatter.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Root => "root",
            Self::Year => "year",
            Self::Month => "month",
            Self::Day => "day",
            Self::Hour => "hour",
        }
    }
}

/// A single node in the summary tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeNode {
    /// Path-style hierarchical id, e.g. `"2024/03/15/09"` or `"root"`.
    pub node_id: String,
    /// Namespace owning this tree (isolates independent trees).
    pub namespace: String,
    /// Hierarchical level this node sits at.
    pub level: NodeLevel,
    /// Id of the parent node; `None` only for the root.
    pub parent_id: Option<String>,
    /// Rolled-up summary text for this node.
    pub summary: String,
    /// Estimated token count of [`Self::summary`]; bounded by [`NodeLevel::max_tokens`].
    pub token_count: u32,
    /// Number of direct children rolled into this node.
    pub child_count: u32,
    /// Creation timestamp (UTC).
    pub created_at: DateTime<Utc>,
    /// Last-update timestamp (UTC).
    pub updated_at: DateTime<Utc>,
    /// Optional opaque metadata blob; omitted from serialization when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<String>,
}

/// Metadata about an entire tree within a namespace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeStatus {
    /// Namespace the tree belongs to.
    pub namespace: String,
    /// Total number of nodes across all levels.
    pub total_nodes: u64,
    /// Number of populated levels (tree height).
    pub depth: u32,
    /// Timestamp of the earliest ingested entry, if any.
    pub oldest_entry: Option<DateTime<Utc>>,
    /// Timestamp of the most recent ingested entry, if any.
    pub newest_entry: Option<DateTime<Utc>>,
    /// When the tree was last (re)built or sealed.
    pub last_run_at: Option<DateTime<Utc>>,
}

/// Input for appending raw content to the ingestion buffer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestRequest {
    /// Target namespace to append content into.
    pub namespace: String,
    /// Raw content to buffer for summarization.
    pub content: String,
    /// Event time used to derive the hour leaf; defaults to ingestion time when absent.
    #[serde(default)]
    pub timestamp: Option<DateTime<Utc>>,
    /// Optional structured metadata carried alongside the content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

/// Result of a tree query at a specific node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryResult {
    /// The node addressed by the query.
    pub node: TreeNode,
    /// Direct children of [`Self::node`], for drill-down navigation.
    pub children: Vec<TreeNode>,
}

/// Rough token estimate: ~4 characters per token.
pub fn estimate_tokens(text: &str) -> u32 {
    (text.len() as u32).div_ceil(4)
}

/// Derive the parent node ID from a node ID.
pub fn derive_parent_id(node_id: &str) -> Option<String> {
    if node_id == "root" {
        return None;
    }
    match node_id.rfind('/') {
        Some(pos) => Some(node_id[..pos].to_string()),
        None => Some("root".to_string()),
    }
}

/// Determine the `NodeLevel` from a node ID string.
pub fn level_from_node_id(node_id: &str) -> NodeLevel {
    if node_id == "root" {
        return NodeLevel::Root;
    }
    match node_id.matches('/').count() {
        0 => NodeLevel::Year,
        1 => NodeLevel::Month,
        2 => NodeLevel::Day,
        _ => NodeLevel::Hour,
    }
}

/// Derive all ancestor node IDs from a timestamp (hour through root).
/// Returns `(hour_id, day_id, month_id, year_id, root_id)`.
pub fn derive_node_ids(ts: &DateTime<Utc>) -> (String, String, String, String, String) {
    let year = format!("{}", ts.year());
    let month = format!("{}/{:02}", ts.year(), ts.month());
    let day = format!("{}/{:02}/{:02}", ts.year(), ts.month(), ts.day());
    let hour = format!(
        "{}/{:02}/{:02}/{:02}",
        ts.year(),
        ts.month(),
        ts.day(),
        ts.hour()
    );
    (hour, day, month, year, "root".to_string())
}

/// Convert a node ID to a relative file path within the tree directory.
pub fn node_id_to_path(node_id: &str) -> PathBuf {
    if node_id == "root" {
        return PathBuf::from("root.md");
    }
    let level = level_from_node_id(node_id);
    if level.is_leaf() {
        PathBuf::from(format!("{node_id}.md"))
    } else {
        PathBuf::from(node_id).join("summary.md")
    }
}

#[cfg(test)]
#[path = "types_tests.rs"]
mod tests;
