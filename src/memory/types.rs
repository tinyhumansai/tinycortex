//! Core public data contracts for the TinyCortex memory engine.
//!
//! These types are the stable surface shared across every layer (storage,
//! ingestion, retrieval, RPC). They are pure data — no storage side effects,
//! no interior mutability, freely `Clone`/`Send`/`Sync` — and are ported
//! faithfully from OpenHuman's `memory` and `memory_store` modules so wire
//! formats (snake_case enum strings, serde defaults) stay byte-compatible when
//! OpenHuman imports this crate.
//!
//! ## Wire-compatibility contract
//!
//! Every `#[serde(rename_all = "snake_case")]` enum here has its variant
//! strings persisted in on-disk indexes (SQLite columns, markdown frontmatter)
//! and/or sent over the RPC boundary. Renaming a variant, or a struct field
//! that lacks `#[serde(default)]`, is a breaking change for any host reading
//! previously-written data. When adding a field, prefer `#[serde(default)]` so
//! older persisted rows continue to deserialize.
//!
//! ## Fail-closed provenance
//!
//! [`MemoryTaint`] is the one field in this module with a safety-relevant
//! default: it decodes unknown/corrupt persisted strings as
//! [`MemoryTaint::ExternalSync`] rather than [`MemoryTaint::Internal`], so a
//! caller that forgets to persist taint, or an index that has drifted, fails
//! toward *more* restrictive tool-use policy rather than less.

use serde::{Deserialize, Serialize};

/// Default namespace used when a caller passes no explicit namespace.
pub const GLOBAL_NAMESPACE: &str = "global";

/// Provenance / trust signal attached to a memory entry.
///
/// Drives downstream policy — most importantly whether automation whose context
/// contains this content may invoke external-effect tools. Defaults to
/// [`MemoryTaint::Internal`] so legacy rows (no persisted taint column) and all
/// in-memory defaults are conservatively trusted as user-driven content.
///
/// Sync paths that ingest text from third-party services (Gmail / Slack /
/// Notion / Composio / MCP / …) MUST set this to [`MemoryTaint::ExternalSync`]
/// at write time so callers can refuse external-effect tools on tainted context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MemoryTaint {
    /// User-driven memory (chat, manual remember, internal heuristics).
    #[default]
    Internal,
    /// Content ingested from an external sync source.
    ExternalSync,
}

impl Serialize for MemoryTaint {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_db_str())
    }
}

impl<'de> Deserialize<'de> for MemoryTaint {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Ok(Self::from_db_str(&raw))
    }
}

impl MemoryTaint {
    /// Serialised form used by the SQLite `memory_docs.taint` column.
    ///
    /// # Examples
    ///
    /// ```
    /// use tinycortex::memory::types::MemoryTaint;
    ///
    /// assert_eq!(MemoryTaint::Internal.as_db_str(), "internal");
    /// assert_eq!(MemoryTaint::ExternalSync.as_db_str(), "external_sync");
    /// ```
    pub fn as_db_str(&self) -> &'static str {
        match self {
            Self::Internal => "internal",
            Self::ExternalSync => "external_sync",
        }
    }

    /// Reverse of [`Self::as_db_str`]. Unknown values fail closed to the more
    /// restrictive [`MemoryTaint::ExternalSync`] so policy gates refuse
    /// external-effect tools on content of unknown provenance.
    ///
    /// Note this is *not* a strict inverse of [`Self::as_db_str`]: it never
    /// errors, so a malformed or unexpected `raw` string (empty, wrong case,
    /// truncated by a partial write, …) silently maps to
    /// [`MemoryTaint::ExternalSync`] rather than surfacing as a parse failure.
    ///
    /// # Examples
    ///
    /// ```
    /// use tinycortex::memory::types::MemoryTaint;
    ///
    /// assert_eq!(MemoryTaint::from_db_str("internal"), MemoryTaint::Internal);
    /// assert_eq!(MemoryTaint::from_db_str("external_sync"), MemoryTaint::ExternalSync);
    /// // Unrecognised input fails closed rather than erroring.
    /// assert_eq!(MemoryTaint::from_db_str("garbage"), MemoryTaint::ExternalSync);
    /// ```
    pub fn from_db_str(raw: &str) -> Self {
        match raw {
            "internal" => Self::Internal,
            "external_sync" => Self::ExternalSync,
            _ => Self::ExternalSync,
        }
    }
}

/// Categories used to organize and filter memories by nature and lifecycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryCategory {
    /// Long-term foundational facts, user preferences, permanent decisions.
    Core,
    /// Temporal logs reflecting daily activities or ephemeral state.
    Daily,
    /// Contextual information derived from active conversations.
    Conversation,
    /// A user- or system-defined custom category.
    Custom(String),
}

/// The stable wire/display representation uses the built-in labels directly
/// and prefixes custom values with `custom:`. The prefix keeps
/// `Custom("core")` distinct from [`MemoryCategory::Core`] and makes Display,
/// serde, and [`std::str::FromStr`] true inverses.
impl std::fmt::Display for MemoryCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Core => write!(f, "core"),
            Self::Daily => write!(f, "daily"),
            Self::Conversation => write!(f, "conversation"),
            Self::Custom(name) => write!(f, "custom:{name}"),
        }
    }
}

impl std::str::FromStr for MemoryCategory {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "core" => Ok(Self::Core),
            "daily" => Ok(Self::Daily),
            "conversation" => Ok(Self::Conversation),
            value if value.starts_with("custom:") && value.len() > "custom:".len() => {
                Ok(Self::Custom(value["custom:".len()..].to_string()))
            }
            _ => Err(format!("unknown memory category: {value}")),
        }
    }
}

impl Serialize for MemoryCategory {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for MemoryCategory {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

/// A single stored memory entry with associated metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    /// Unique identifier (usually a UUID).
    pub id: String,
    /// Key or title associated with this memory.
    pub key: String,
    /// Actual content / value of the memory.
    pub content: String,
    /// Optional namespace for logical separation.
    #[serde(default)]
    pub namespace: Option<String>,
    /// Organizational category.
    pub category: MemoryCategory,
    /// ISO 8601 timestamp of create / last-update.
    pub timestamp: String,
    /// Optional session scope.
    pub session_id: Option<String>,
    /// Optional relevance / confidence score (typically 0.0–1.0).
    pub score: Option<f64>,
    /// Provenance taint (see [`MemoryTaint`]). Absent on legacy JSON, in which
    /// case it defaults to [`MemoryTaint::Internal`]; unknown persisted string
    /// values decode as [`MemoryTaint::ExternalSync`].
    #[serde(default)]
    pub taint: MemoryTaint,
}

/// Optional filters for recall.
#[derive(Debug, Default, Clone)]
pub struct RecallOpts<'a> {
    /// Restrict recall to this namespace; `None` falls back to [`GLOBAL_NAMESPACE`].
    pub namespace: Option<&'a str>,
    /// Restrict recall to entries of this category.
    pub category: Option<MemoryCategory>,
    /// Restrict recall to entries scoped to this session.
    pub session_id: Option<&'a str>,
    /// Drop hits scoring below this threshold (typically 0.0–1.0).
    pub min_score: Option<f64>,
    /// When `true`, include conversational hits from other sessions in the same
    /// workspace alongside the namespace recall.
    pub cross_session: bool,
}

/// Summary row for agent-side namespace discovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamespaceSummary {
    /// Namespace identifier.
    pub namespace: String,
    /// Number of memory entries currently stored in the namespace.
    pub count: usize,
    /// RFC3339 timestamp of the most recent update in the namespace, if any.
    pub last_updated: Option<String>,
}

/// Input payload for upserting a namespace-scoped memory document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamespaceDocumentInput {
    /// Target namespace for the document.
    pub namespace: String,
    /// Stable upsert key; reusing a key updates the existing document.
    pub key: String,
    /// Human-readable title.
    pub title: String,
    /// Document body.
    pub content: String,
    /// Origin of the content (e.g. `chat`, `gmail`, `notion`).
    pub source_type: String,
    /// Caller-defined priority label.
    pub priority: String,
    /// Free-form tags for filtering.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Arbitrary structured metadata carried alongside the document.
    #[serde(default)]
    pub metadata: serde_json::Value,
    /// Category label (see [`MemoryCategory`] wire strings).
    pub category: String,
    /// Optional session scope.
    #[serde(default)]
    pub session_id: Option<String>,
    /// Explicit document id; generated when absent.
    #[serde(default)]
    pub document_id: Option<String>,
    /// Provenance taint; defaults to [`MemoryTaint::Internal`] for legacy JSON
    /// missing this field. Unknown persisted string values decode as
    /// [`MemoryTaint::ExternalSync`].
    #[serde(default)]
    pub taint: MemoryTaint,
}

/// One ranked retrieval result for a namespace text query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamespaceQueryResult {
    /// Upsert key of the matched document.
    pub key: String,
    /// Matched content.
    pub content: String,
    /// Relevance score for this hit.
    pub score: f64,
    /// Category label of the matched document.
    pub category: String,
    /// Provenance taint; unknown persisted values decode as `external_sync`.
    #[serde(default)]
    pub taint: MemoryTaint,
}

/// Discriminator for the kind of stored memory item a hit refers to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryItemKind {
    /// A namespace-scoped memory document (`memory_docs` row).
    Document,
    /// A key/value record.
    Kv,
    /// An episodic / conversational memory.
    Episodic,
    /// A discrete event entry.
    Event,
}

/// Persisted form of a memory document as stored in `memory_docs`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredMemoryDocument {
    /// Unique document id.
    pub document_id: String,
    /// Owning namespace.
    pub namespace: String,
    /// Stable upsert key.
    pub key: String,
    /// Human-readable title.
    pub title: String,
    /// Document body.
    pub content: String,
    /// Origin of the content (e.g. `chat`, `gmail`).
    pub source_type: String,
    /// Caller-defined priority label.
    pub priority: String,
    /// Free-form tags.
    pub tags: Vec<String>,
    /// Arbitrary structured metadata.
    pub metadata: serde_json::Value,
    /// Category label.
    pub category: String,
    /// Optional session scope.
    pub session_id: Option<String>,
    /// Creation time as a Unix timestamp (seconds).
    pub created_at: f64,
    /// Last-update time as a Unix timestamp (seconds).
    pub updated_at: f64,
    /// Path, relative to the vault root, of the authoritative markdown file.
    pub markdown_rel_path: String,
    /// Provenance taint; unknown persisted values decode as `external_sync`.
    #[serde(default)]
    pub taint: MemoryTaint,
}

/// A single KV row, namespace-scoped or global (when `namespace` is `None`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryKvRecord {
    /// Owning namespace, or `None` for a global row.
    pub namespace: Option<String>,
    /// KV key.
    pub key: String,
    /// Stored JSON value.
    pub value: serde_json::Value,
    /// Last-update time as a Unix timestamp (seconds).
    pub updated_at: f64,
}

/// A graph edge (subject — predicate → object) plus accumulated evidence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphRelationRecord {
    /// Owning namespace, or `None` for a global relation.
    pub namespace: Option<String>,
    /// Edge subject (head entity).
    pub subject: String,
    /// Relation type linking subject to object.
    pub predicate: String,
    /// Edge object (tail entity).
    pub object: String,
    /// Arbitrary structured attributes attached to the edge.
    pub attrs: serde_json::Value,
    /// Last-update time as a Unix timestamp (seconds).
    pub updated_at: f64,
    /// Number of independent observations supporting this edge.
    pub evidence_count: u32,
    /// Optional ordering hint among sibling relations.
    pub order_index: Option<i64>,
    /// Documents that contributed evidence for this edge.
    pub document_ids: Vec<String>,
    /// Chunks that contributed evidence for this edge.
    pub chunk_ids: Vec<String>,
}

/// Per-signal contribution to a hit's final score, for ranking explainers.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RetrievalScoreBreakdown {
    /// Lexical / keyword match contribution.
    pub keyword_relevance: f64,
    /// Vector (cosine) similarity contribution.
    pub vector_similarity: f64,
    /// Graph-proximity contribution.
    pub graph_relevance: f64,
    /// Episodic-recall contribution.
    pub episodic_relevance: f64,
    /// Recency contribution.
    pub freshness: f64,
    /// Weighted combination of the above signals; the value used for ranking.
    pub final_score: f64,
}

/// A single ranked retrieval hit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamespaceMemoryHit {
    /// Identifier of the matched item (interpretation depends on [`Self::kind`]).
    pub id: String,
    /// Which kind of stored item this hit refers to.
    pub kind: MemoryItemKind,
    /// Owning namespace.
    pub namespace: String,
    /// Upsert key of the matched item.
    pub key: String,
    /// Title, when the item has one.
    pub title: Option<String>,
    /// Matched content.
    pub content: String,
    /// Category label.
    pub category: String,
    /// Origin of the content, when known.
    pub source_type: Option<String>,
    /// Last-update time as a Unix timestamp (seconds).
    pub updated_at: f64,
    /// Final ranking score; mirrors [`RetrievalScoreBreakdown::final_score`].
    pub score: f64,
    /// Per-signal explanation of how [`Self::score`] was derived.
    pub score_breakdown: RetrievalScoreBreakdown,
    /// Source document id, when the hit resolves to a document.
    #[serde(default)]
    pub document_id: Option<String>,
    /// Source chunk id, when the hit resolves to a chunk.
    #[serde(default)]
    pub chunk_id: Option<String>,
    /// Graph relations that reinforced this hit's ranking.
    #[serde(default)]
    pub supporting_relations: Vec<GraphRelationRecord>,
    /// Provenance taint; unknown persisted values decode as `external_sync`.
    #[serde(default)]
    pub taint: MemoryTaint,
}

/// Aggregated retrieval result for a namespace: rendered context plus hits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamespaceRetrievalContext {
    /// Namespace the retrieval ran against.
    pub namespace: String,
    /// Originating query text, if any.
    pub query: Option<String>,
    /// Rendered, ready-to-inject context assembled from [`Self::hits`].
    pub context_text: String,
    /// Ranked hits backing the rendered context.
    pub hits: Vec<NamespaceMemoryHit>,
}

#[cfg(test)]
#[path = "types_tests.rs"]
mod tests;
