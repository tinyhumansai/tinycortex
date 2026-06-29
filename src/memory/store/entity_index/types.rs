//! Types for the entity occurrence index.
//!
//! Ported from OpenHuman's `memory_tree::score::extract::types::EntityKind`,
//! `score::resolver::CanonicalEntity`, and `score::store::EntityHit`. The
//! occurrence index maps a canonical entity id to every tree node (chunk or
//! summary) it appears in — it is NOT the markdown contact registry.

use serde::{Deserialize, Serialize};

/// Classification of an extracted span.
///
/// Split into two categories:
/// - **Mechanical** — regex finds these deterministically (high precision).
/// - **Semantic** — model-based named references (Person, Organization, …).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum EntityKind {
    // Mechanical
    Email,
    Url,
    Handle,
    Hashtag,
    // Semantic — emitted by the LLM extractor.
    Person,
    Organization,
    Location,
    Event,
    Product,
    /// Temporal expressions: "Friday", "Q2 2026", "EOD tomorrow".
    Datetime,
    /// Tools / frameworks / languages / services: "Rust", "OAuth", "Slack API".
    Technology,
    /// Code / ticket / doc references: "PR #934", "src/...", "OH-42".
    Artifact,
    /// Amounts / metrics / money: "$5K", "20/min", "10k tokens".
    Quantity,
    Misc,
    /// Scorer-surfaced themes promoted into the canonical entity stream.
    Topic,
}

impl EntityKind {
    /// Snake-case wire string for serialisation and SQL storage.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Email => "email",
            Self::Url => "url",
            Self::Handle => "handle",
            Self::Hashtag => "hashtag",
            Self::Person => "person",
            Self::Organization => "organization",
            Self::Location => "location",
            Self::Event => "event",
            Self::Product => "product",
            Self::Datetime => "datetime",
            Self::Technology => "technology",
            Self::Artifact => "artifact",
            Self::Quantity => "quantity",
            Self::Misc => "misc",
            Self::Topic => "topic",
        }
    }

    /// Inverse of [`Self::as_str`]; returns `Err` for unknown wire strings.
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "email" => Ok(Self::Email),
            "url" => Ok(Self::Url),
            "handle" => Ok(Self::Handle),
            "hashtag" => Ok(Self::Hashtag),
            "person" => Ok(Self::Person),
            "organization" => Ok(Self::Organization),
            "location" => Ok(Self::Location),
            "event" => Ok(Self::Event),
            "product" => Ok(Self::Product),
            "datetime" => Ok(Self::Datetime),
            "technology" => Ok(Self::Technology),
            "artifact" => Ok(Self::Artifact),
            "quantity" => Ok(Self::Quantity),
            "misc" => Ok(Self::Misc),
            "topic" => Ok(Self::Topic),
            other => Err(format!("unknown entity kind: {other}")),
        }
    }

    /// Whether this kind comes from deterministic extraction.
    pub fn is_mechanical(self) -> bool {
        matches!(self, Self::Email | Self::Url | Self::Handle | Self::Hashtag)
    }
}

/// A canonicalised entity occurrence ready to be indexed.
///
/// Same surface form (after normalisation) → same `canonical_id` regardless
/// of how many times it appears. One row per occurrence preserves source spans.
#[derive(Clone, Debug, PartialEq)]
pub struct CanonicalEntity {
    /// Stable id following the `"<kind>:<value>"` convention.
    pub canonical_id: String,
    /// Entity classification.
    pub kind: EntityKind,
    /// Surface form as it appeared in the source text.
    pub surface: String,
    /// Character offset `[span_start, span_end)` into the source text.
    pub span_start: u32,
    /// End character offset.
    pub span_end: u32,
    /// Extractor confidence `[0.0, 1.0]` (regex = 1.0).
    pub score: f32,
}

/// Result row from a lookup against the entity index.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EntityHit {
    /// Canonical entity id (`"<kind>:<value>"`).
    pub entity_id: String,
    /// Tree node (chunk or summary) the entity appears in.
    pub node_id: String,
    /// `"leaf"` / `"summary"` — the kind of node.
    pub node_kind: String,
    /// Parsed entity kind.
    pub entity_kind: EntityKind,
    /// Stored surface form (or the canonical id for summary rows).
    pub surface: String,
    /// Occurrence score carried from the source entity / node.
    pub score: f32,
    /// Node timestamp in epoch milliseconds (sort key, newest first).
    pub timestamp_ms: i64,
    /// Owning tree id, when known.
    pub tree_id: Option<String>,
    /// True when the canonical id matched a self-identity at index time.
    #[serde(default)]
    pub is_user: bool,
}
