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
    Root,
    Year,
    Month,
    Day,
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
    pub node_id: String,
    pub namespace: String,
    pub level: NodeLevel,
    pub parent_id: Option<String>,
    pub summary: String,
    pub token_count: u32,
    pub child_count: u32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<String>,
}

/// Metadata about an entire tree within a namespace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeStatus {
    pub namespace: String,
    pub total_nodes: u64,
    pub depth: u32,
    pub oldest_entry: Option<DateTime<Utc>>,
    pub newest_entry: Option<DateTime<Utc>>,
    pub last_run_at: Option<DateTime<Utc>>,
}

/// Input for appending raw content to the ingestion buffer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestRequest {
    pub namespace: String,
    pub content: String,
    #[serde(default)]
    pub timestamp: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

/// Result of a tree query at a specific node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryResult {
    pub node: TreeNode,
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
