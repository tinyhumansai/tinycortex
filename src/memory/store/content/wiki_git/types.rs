//! Data types for the wiki git mirror: the per-summary metadata carried in a
//! commit and the batch that ties a tree seal to its entries.

use chrono::{DateTime, Utc};

/// Metadata for one summary node included in a wiki git commit.
#[derive(Clone, Debug)]
pub struct SummaryCommitEntry {
    pub summary_id: String,
    pub content_path: String,
    pub level: u32,
    pub child_count: usize,
    pub token_count: u32,
    pub time_range_start: DateTime<Utc>,
    pub time_range_end: DateTime<Utc>,
}

/// Metadata for one tree seal represented as a wiki git commit.
#[derive(Clone, Debug)]
pub struct SummaryCommitBatch {
    pub reason: String,
    pub tree_id: String,
    pub tree_scope: String,
    pub entries: Vec<SummaryCommitEntry>,
}
