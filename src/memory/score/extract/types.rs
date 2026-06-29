//! Types produced by entity extractors (Phase 2 / #708).
//!
//! The pipeline runs one or more [`super::EntityExtractor`] impls over each
//! admitted chunk and collects all their output into [`ExtractedEntities`].

use serde::{Deserialize, Serialize};

/// Classification of an extracted span.
///
/// Split into two categories:
/// - **Mechanical** — regex finds these deterministically. Stable, high precision,
///   limited recall. These are "identifiers" (pointers), not "entities"
///   in the semantic sense.
/// - **Semantic** — model-based (LLM). Named references to real-world objects:
///   Person, Organization, Location, Event, Product.
///
/// Phase 2 ships with mechanical-only; semantic variants are populated by the
/// trait-abstracted LLM extractor when one is configured.
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
    /// Temporal expressions: "Friday", "Q2 2026", "EOD tomorrow", "next sprint".
    Datetime,
    /// Tools / frameworks / programming languages / services:
    /// "Rust", "OAuth", "Slack API", "nomic-embed".
    Technology,
    /// Code / ticket / doc references that point at something addressable:
    /// "PR #934", "src/openhuman/...", "OH-42", "ab7da2e2".
    Artifact,
    /// Amounts / metrics / money: "$5K", "20/min", "10k tokens", "52 chunks".
    Quantity,
    Misc,
    // Thematic — scorer-surfaced topics (hashtag-like short phrases or
    // LLM-extracted themes). Promoted into the canonical entity stream
    // by the resolver so topic trees can route on themes the
    // same way they route on people/orgs. A chunk saying "Phoenix
    // migration ships Friday" emits `topic:phoenix` and `topic:migration`
    // in addition to any emails/hashtags the mechanical extractors find.
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

/// One extracted span from a chunk's content.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExtractedEntity {
    pub kind: EntityKind,
    /// Surface form as it appears in the chunk.
    pub text: String,
    /// Character offsets `[start, end)` into the chunk text.
    pub span_start: u32,
    pub span_end: u32,
    /// Extractor confidence `[0.0, 1.0]`. Regex = 1.0; model-based = output.
    pub score: f32,
}

/// Topic candidate (hashtag-style or summariser-labeled).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExtractedTopic {
    /// Normalised topic text (lowercase, no leading `#`).
    pub label: String,
    pub score: f32,
}

/// Aggregate output of one or more extractors on a single chunk.
///
/// `llm_importance` and `llm_importance_reason` are populated by extractors
/// that piggyback an importance rating on their NER call (see
/// [`super::llm::LlmEntityExtractor`]). Cheap regex extractors leave them
/// `None`; downstream signal compute treats `None` as "no LLM signal" and
/// the weighted combine zeroes that contribution out so behaviour matches
/// pre-LLM Phase 2 exactly when LLM is disabled.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ExtractedEntities {
    pub entities: Vec<ExtractedEntity>,
    pub topics: Vec<ExtractedTopic>,
    /// Optional LLM-rated importance in `[0.0, 1.0]` for this chunk.
    /// `None` means no LLM signal is available.
    #[serde(default)]
    pub llm_importance: Option<f32>,
    /// One-line audit trail from the LLM explaining the importance rating.
    /// Used purely for diagnostics; never feeds back into scoring.
    #[serde(default)]
    pub llm_importance_reason: Option<String>,
}

impl ExtractedEntities {
    /// True when neither entities nor topics were extracted.
    pub fn is_empty(&self) -> bool {
        self.entities.is_empty() && self.topics.is_empty()
    }

    /// Count of unique `(kind, text)` pairs, case-insensitive. Used as a scoring signal.
    pub fn unique_entity_count(&self) -> usize {
        use std::collections::HashSet;
        // Only the cardinality matters, so a hash set counts the distinct
        // `(kind, text)` pairs without paying for ordered insertion.
        self.entities
            .iter()
            .map(|e| (e.kind, e.text.to_lowercase()))
            .collect::<HashSet<_>>()
            .len()
    }

    /// Merge another extractor's output into this one.
    ///
    /// Deduplicates entities by `(kind, normalised_text, span_start)` and
    /// topics by `label` so the same match from two extractors doesn't get
    /// double-counted.
    ///
    /// LLM importance signals merge by **maximum** — if either side rated
    /// the chunk as important, the merged result keeps that higher rating.
    /// The reason from whichever side won the max wins; if they tied or
    /// both are absent, the non-empty one (if any) is kept.
    pub fn merge(&mut self, other: ExtractedEntities) {
        use std::collections::HashSet;
        // Both sets are membership-only dedup guards — the surviving order is
        // the existing `Vec` push order, never the set's — so a hash set keeps
        // the merge result identical while dropping the ordered-key overhead.
        let mut seen: HashSet<(EntityKind, String, u32)> = self
            .entities
            .iter()
            .map(|e| (e.kind, e.text.to_lowercase(), e.span_start))
            .collect();
        for e in other.entities {
            let key = (e.kind, e.text.to_lowercase(), e.span_start);
            if seen.insert(key) {
                self.entities.push(e);
            }
        }
        let mut topic_seen: HashSet<String> = self.topics.iter().map(|t| t.label.clone()).collect();
        for t in other.topics {
            if topic_seen.insert(t.label.clone()) {
                self.topics.push(t);
            }
        }

        // Merge LLM importance: max wins, reason follows the max.
        match (self.llm_importance, other.llm_importance) {
            (Some(a), Some(b)) if b > a => {
                self.llm_importance = Some(b);
                self.llm_importance_reason = other.llm_importance_reason;
            }
            (None, Some(b)) => {
                self.llm_importance = Some(b);
                self.llm_importance_reason = other.llm_importance_reason;
            }
            // self.a >= other.b OR other has nothing — keep self
            _ => {
                if self.llm_importance_reason.is_none() {
                    self.llm_importance_reason = other.llm_importance_reason;
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "types_tests.rs"]
mod tests;
