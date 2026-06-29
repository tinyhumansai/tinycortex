//! Core persisted types for summary trees — ported from OpenHuman's
//! `memory_store/trees/types.rs`.
//!
//! These types sit on top of the chunk leaves. A [`Tree`] groups leaves under
//! one scope (e.g. one chat channel, one email account). When a [`Buffer`] at
//! some level accumulates enough tokens (L0) or siblings (L≥1), its contents
//! seal into a [`SummaryNode`] at level+1 and the buffer clears. Summary nodes
//! are immutable once emitted.
//!
//! Budgets are re-exported from [`crate::memory::config`] so there is a single
//! source of truth for the tunables (`INPUT_TOKEN_BUDGET`, `OUTPUT_TOKEN_BUDGET`,
//! `SUMMARY_FANOUT`); the engine reads them from [`crate::memory::config::TreeConfig`]
//! at runtime, these consts are the defaults/test references.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub use crate::memory::config::{INPUT_TOKEN_BUDGET, OUTPUT_TOKEN_BUDGET, SUMMARY_FANOUT};

/// Default age at which a non-empty buffer is force-sealed even under budget,
/// in seconds. Mirrors [`crate::memory::config::DEFAULT_FLUSH_AGE_SECS`] but as
/// `i64` to compose with `chrono::Duration::seconds`.
pub const DEFAULT_FLUSH_AGE_SECS: i64 = crate::memory::config::DEFAULT_FLUSH_AGE_SECS as i64;

/// What kind of tree this is. Source trees live per ingest source; topic and
/// global trees share the same schema.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum TreeKind {
    /// One tree per ingest source (e.g. `chat:slack:#eng`, `email:gmail:user`).
    Source,
    /// Per-entity/topic tree.
    Topic,
    /// Cross-source daily digest tree.
    Global,
}

impl TreeKind {
    /// Stable lowercase form used in SQL discriminator columns and ids.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Topic => "topic",
            Self::Global => "global",
        }
    }

    /// Inverse of [`Self::as_str`] — parse back from a discriminator string.
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "source" => Ok(Self::Source),
            "topic" => Ok(Self::Topic),
            "global" => Ok(Self::Global),
            other => Err(format!("unknown tree kind: {other}")),
        }
    }
}

/// Activity state of a tree. Archived trees stay queryable but don't accept new
/// leaves.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TreeStatus {
    Active,
    Archived,
}

impl TreeStatus {
    /// Stable lowercase form used as the SQL discriminator value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Archived => "archived",
        }
    }

    /// Inverse of [`Self::as_str`] — parse from the SQL discriminator.
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "active" => Ok(Self::Active),
            "archived" => Ok(Self::Archived),
            other => Err(format!("unknown tree status: {other}")),
        }
    }
}

/// One summary-tree instance.
///
/// `root_id` is `None` until the first seal emits an L1 node. `max_level`
/// tracks the highest level that has ever sealed; `root_id` points at the
/// current top node at that level.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Tree {
    pub id: String,
    pub kind: TreeKind,
    /// Logical identifier for what the tree covers (source id, entity id, or
    /// the literal `"global"`).
    pub scope: String,
    pub root_id: Option<String>,
    pub max_level: u32,
    pub status: TreeStatus,
    pub created_at: DateTime<Utc>,
    pub last_sealed_at: Option<DateTime<Utc>>,
}

/// A sealed summary node — one level above raw leaves.
///
/// `child_ids` points at the concrete children that were in the buffer when
/// this node sealed. For L1 nodes those are leaf `chunk.id`s; for L2+ they are
/// lower-level summary ids. The relation is fixed at seal time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SummaryNode {
    pub id: String,
    pub tree_id: String,
    pub tree_kind: TreeKind,
    /// 1 for summaries over raw leaves, 2 over L1 summaries, and so on.
    pub level: u32,
    pub parent_id: Option<String>,
    pub child_ids: Vec<String>,
    /// Summariser output.
    pub content: String,
    pub token_count: u32,
    /// Curated subset of children's entity canonical-ids.
    pub entities: Vec<String>,
    /// Curated topic labels.
    pub topics: Vec<String>,
    pub time_range_start: DateTime<Utc>,
    pub time_range_end: DateTime<Utc>,
    /// Max of children's scores at seal time.
    pub score: f32,
    pub sealed_at: DateTime<Utc>,
    /// Tombstone flag — summaries are immutable, this stays `false` on new seals.
    pub deleted: bool,
    /// Optional summary-content embedding for semantic rerank. `None` on reads
    /// where the blob column is NULL (the engine persists embeddings to the
    /// per-model sidecar table, leaving this legacy column NULL).
    #[serde(default)]
    pub embedding: Option<Vec<f32>>,
    /// Document identity this node belongs to, for document source trees.
    #[serde(default)]
    pub doc_id: Option<String>,
    /// Document version this node was sealed for, as epoch-milliseconds.
    #[serde(default)]
    pub version_ms: Option<i64>,
}

/// Unsealed frontier at a given `(tree_id, level)`. One row per level per tree.
/// `oldest_at` is `None` when the buffer is empty; used by the time-based flush
/// trigger.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Buffer {
    pub tree_id: String,
    pub level: u32,
    pub item_ids: Vec<String>,
    pub token_sum: i64,
    pub oldest_at: Option<DateTime<Utc>>,
}

impl Buffer {
    /// Empty buffer at the given key.
    pub fn empty(tree_id: &str, level: u32) -> Self {
        Self {
            tree_id: tree_id.to_string(),
            level,
            item_ids: Vec::new(),
            token_sum: 0,
            oldest_at: None,
        }
    }

    /// True when the buffer holds no pending items.
    pub fn is_empty(&self) -> bool {
        self.item_ids.is_empty()
    }

    /// Whether the buffer's oldest item is older than `max_age`. Returns
    /// `false` for an empty buffer.
    pub fn is_stale(&self, now: DateTime<Utc>, max_age: chrono::Duration) -> bool {
        match self.oldest_at {
            Some(ts) => now.signed_duration_since(ts) > max_age,
            None => false,
        }
    }
}

// ── Topic-tree hotness ──────────────────────────────────────────────────────

/// Hotness threshold above which a topic tree is materialised for an entity.
pub const TOPIC_CREATION_THRESHOLD: f32 = 10.0;

/// Hotness threshold below which a topic tree becomes an archive candidate.
pub const TOPIC_ARCHIVE_THRESHOLD: f32 = 2.0;

/// How often (in ingests touching the entity) to recompute hotness fully.
pub const TOPIC_RECHECK_EVERY: u32 = 100;

/// Input record fed to the hotness math.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EntityIndexStats {
    pub mention_count_30d: u32,
    pub distinct_sources: u32,
    pub last_seen_ms: Option<i64>,
    pub query_hits_30d: u32,
    pub graph_centrality: Option<f32>,
}

/// Row persisted in `mem_tree_entity_hotness`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HotnessCounters {
    pub entity_id: String,
    pub mention_count_30d: u32,
    pub distinct_sources: u32,
    pub last_seen_ms: Option<i64>,
    pub query_hits_30d: u32,
    pub graph_centrality: Option<f32>,
    pub ingests_since_check: u32,
    pub last_hotness: Option<f32>,
    pub last_updated_ms: i64,
}

impl HotnessCounters {
    pub fn fresh(entity_id: &str, now_ms: i64) -> Self {
        Self {
            entity_id: entity_id.to_string(),
            mention_count_30d: 0,
            distinct_sources: 0,
            last_seen_ms: None,
            query_hits_30d: 0,
            graph_centrality: None,
            ingests_since_check: 0,
            last_hotness: None,
            last_updated_ms: now_ms,
        }
    }

    pub fn stats(&self) -> EntityIndexStats {
        EntityIndexStats {
            mention_count_30d: self.mention_count_30d,
            distinct_sources: self.distinct_sources,
            last_seen_ms: self.last_seen_ms,
            query_hits_30d: self.query_hits_30d,
            graph_centrality: self.graph_centrality,
        }
    }
}

#[cfg(test)]
#[path = "types_tests.rs"]
mod tests;
