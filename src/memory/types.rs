//! Core public data contracts for the TinyCortex memory engine.
//!
//! These types are the stable surface shared across every layer (storage,
//! ingestion, retrieval, RPC). They are pure data — no storage side effects —
//! and are ported faithfully from OpenHuman's `memory` and `memory_store`
//! modules so wire formats (snake_case enum strings, serde defaults) stay
//! byte-compatible when OpenHuman imports this crate.

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MemoryTaint {
    /// User-driven memory (chat, manual remember, internal heuristics).
    #[default]
    Internal,
    /// Content ingested from an external sync source.
    ExternalSync,
}

impl MemoryTaint {
    /// Serialised form used by the SQLite `memory_docs.taint` column.
    pub fn as_db_str(&self) -> &'static str {
        match self {
            Self::Internal => "internal",
            Self::ExternalSync => "external_sync",
        }
    }

    /// Reverse of [`Self::as_db_str`]. Unknown values fail closed to the more
    /// restrictive [`MemoryTaint::ExternalSync`] so policy gates refuse
    /// external-effect tools on content of unknown provenance.
    pub fn from_db_str(raw: &str) -> Self {
        match raw {
            "internal" => Self::Internal,
            "external_sync" => Self::ExternalSync,
            _ => Self::ExternalSync,
        }
    }
}

/// Categories used to organize and filter memories by nature and lifecycle.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
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

impl std::fmt::Display for MemoryCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Core => write!(f, "core"),
            Self::Daily => write!(f, "daily"),
            Self::Conversation => write!(f, "conversation"),
            Self::Custom(name) => write!(f, "{name}"),
        }
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
    /// Provenance taint. See [`MemoryTaint`].
    #[serde(default)]
    pub taint: MemoryTaint,
}

/// Optional filters for recall.
#[derive(Debug, Default, Clone)]
pub struct RecallOpts<'a> {
    pub namespace: Option<&'a str>,
    pub category: Option<MemoryCategory>,
    pub session_id: Option<&'a str>,
    pub min_score: Option<f64>,
    /// When `true`, include conversational hits from other sessions in the same
    /// workspace alongside the namespace recall.
    pub cross_session: bool,
}

/// Summary row for agent-side namespace discovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamespaceSummary {
    pub namespace: String,
    pub count: usize,
    /// RFC3339 timestamp of the most recent update in the namespace, if any.
    pub last_updated: Option<String>,
}

/// Input payload for upserting a namespace-scoped memory document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamespaceDocumentInput {
    pub namespace: String,
    pub key: String,
    pub title: String,
    pub content: String,
    pub source_type: String,
    pub priority: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub metadata: serde_json::Value,
    pub category: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub document_id: Option<String>,
    /// Provenance taint; defaults to [`MemoryTaint::Internal`] for legacy JSON.
    #[serde(default)]
    pub taint: MemoryTaint,
}

/// One ranked retrieval result for a namespace text query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamespaceQueryResult {
    pub key: String,
    pub content: String,
    pub score: f64,
    pub category: String,
    #[serde(default)]
    pub taint: MemoryTaint,
}

/// Discriminator for the kind of stored memory item a hit refers to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryItemKind {
    Document,
    Kv,
    Episodic,
    Event,
}

/// Persisted form of a memory document as stored in `memory_docs`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredMemoryDocument {
    pub document_id: String,
    pub namespace: String,
    pub key: String,
    pub title: String,
    pub content: String,
    pub source_type: String,
    pub priority: String,
    pub tags: Vec<String>,
    pub metadata: serde_json::Value,
    pub category: String,
    pub session_id: Option<String>,
    pub created_at: f64,
    pub updated_at: f64,
    pub markdown_rel_path: String,
    #[serde(default)]
    pub taint: MemoryTaint,
}

/// A single KV row, namespace-scoped or global (when `namespace` is `None`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryKvRecord {
    pub namespace: Option<String>,
    pub key: String,
    pub value: serde_json::Value,
    pub updated_at: f64,
}

/// A graph edge (subject — predicate → object) plus accumulated evidence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphRelationRecord {
    pub namespace: Option<String>,
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub attrs: serde_json::Value,
    pub updated_at: f64,
    pub evidence_count: u32,
    pub order_index: Option<i64>,
    pub document_ids: Vec<String>,
    pub chunk_ids: Vec<String>,
}

/// Per-signal contribution to a hit's final score, for ranking explainers.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RetrievalScoreBreakdown {
    pub keyword_relevance: f64,
    pub vector_similarity: f64,
    pub graph_relevance: f64,
    pub episodic_relevance: f64,
    pub freshness: f64,
    pub final_score: f64,
}

/// A single ranked retrieval hit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamespaceMemoryHit {
    pub id: String,
    pub kind: MemoryItemKind,
    pub namespace: String,
    pub key: String,
    pub title: Option<String>,
    pub content: String,
    pub category: String,
    pub source_type: Option<String>,
    pub updated_at: f64,
    pub score: f64,
    pub score_breakdown: RetrievalScoreBreakdown,
    #[serde(default)]
    pub document_id: Option<String>,
    #[serde(default)]
    pub chunk_id: Option<String>,
    #[serde(default)]
    pub supporting_relations: Vec<GraphRelationRecord>,
    #[serde(default)]
    pub taint: MemoryTaint,
}

/// Aggregated retrieval result for a namespace: rendered context plus hits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamespaceRetrievalContext {
    pub namespace: String,
    pub query: Option<String>,
    pub context_text: String,
    pub hits: Vec<NamespaceMemoryHit>,
}

#[cfg(test)]
#[path = "types_tests.rs"]
mod tests;
