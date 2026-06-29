//! Standalone documents → canonical Markdown.
//!
//! Document sources are single-record (no grouping): one Notion page, one
//! Drive doc, one meeting-note file. The canonicaliser trims the body and
//! passes it through; if the body is already markdown it is kept verbatim. No
//! leading title header is added — provider / title belong in the content-store
//! front matter.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{normalize_source_ref, CanonicalisedSource};
use crate::memory::chunks::{Metadata, SourceKind};

// ── Serde helpers ────────────────────────────────────────────────────────────

fn default_provider() -> String {
    "unknown".to_string()
}

fn now_utc() -> DateTime<Utc> {
    Utc::now()
}

// ── Input struct ─────────────────────────────────────────────────────────────

/// Adapter input for a single document.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DocumentInput {
    /// Provider name (e.g. `notion`, `drive`, `meeting_notes`). Defaults to
    /// `"unknown"` when absent.
    #[serde(default = "default_provider")]
    pub provider: String,
    /// Document title.
    pub title: String,
    /// Document body (markdown preferred; plain text also accepted).
    pub body: String,
    /// When the document was last modified at the source.
    ///
    /// Accepts epoch-milliseconds integer (back-compat), RFC 3339 / ISO-8601
    /// string, or absent → `Utc::now()`.
    #[serde(
        default = "now_utc",
        deserialize_with = "super::deserialize_flexible_timestamp"
    )]
    pub modified_at: DateTime<Utc>,
    /// Optional pointer back to source (URL, file path, Notion page id).
    #[serde(default)]
    pub source_ref: Option<String>,
}

/// Canonicalise a single document into a [`CanonicalisedSource`]. Returns
/// `Ok(None)` if both the title and body are empty — caller treats as nothing
/// to ingest.
pub fn canonicalise(
    source_id: &str,
    owner: &str,
    tags: &[String],
    doc: DocumentInput,
    path_scope: Option<String>,
) -> Result<Option<CanonicalisedSource>, String> {
    if doc.body.trim().is_empty() && doc.title.trim().is_empty() {
        return Ok(None);
    }

    let mut md = String::new();
    // No leading `# provider — title` header. Provider / title info belongs in
    // the MD front-matter.
    md.push_str(doc.body.trim());
    md.push('\n');

    Ok(Some(CanonicalisedSource {
        markdown: md,
        metadata: Metadata {
            source_kind: SourceKind::Document,
            source_id: source_id.to_string(),
            owner: owner.to_string(),
            timestamp: doc.modified_at,
            time_range: (doc.modified_at, doc.modified_at),
            tags: tags.to_vec(),
            source_ref: normalize_source_ref(doc.source_ref),
            path_scope,
        },
    }))
}

#[cfg(test)]
#[path = "document_tests.rs"]
mod tests;
