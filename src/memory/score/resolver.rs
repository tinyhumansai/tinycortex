//! Entity canonicalisation / cross-platform merge (Phase 2 / #708, V1).
//!
//! Exact-match only: normalises surface forms (lowercase emails, strip leading
//! `@` on handles) and assigns a canonical `entity_id` string.
//!
//! Fuzzy matching (alice-slack ≡ Alice-Discord by soft match) is deferred until
//! we have real entity-graph data — the current implementation handles the
//! mechanical cases cleanly without producing false merges.

use serde::{Deserialize, Serialize};

use crate::memory::score::extract::{EntityKind, ExtractedEntities};

/// Canonicalised entity — same shape as [`super::extract::ExtractedEntity`]
/// plus a stable `canonical_id` suitable for indexing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CanonicalEntity {
    pub canonical_id: String,
    pub kind: EntityKind,
    pub surface: String,
    pub span_start: u32,
    pub span_end: u32,
    pub score: f32,
}

/// Canonicalise a batch of extracted entities.
///
/// Same surface form (after normalisation) → same `canonical_id` regardless of
/// how many times it appears in a chunk. Preserves source spans by emitting one
/// [`CanonicalEntity`] per occurrence.
///
/// Extracted **topics** are also promoted into the canonical stream under
/// [`EntityKind::Topic`] so downstream routing can treat themes as first-class
/// scope alongside people/orgs. Topics have no source span (they're derived
/// from the whole chunk), so `span_start` / `span_end` are both `0` for topic
/// rows — readers should key on `kind` instead of span when span-awareness
/// matters.
pub fn canonicalise(extracted: &ExtractedEntities) -> Vec<CanonicalEntity> {
    let mut out: Vec<CanonicalEntity> = extracted
        .entities
        .iter()
        .map(|e| CanonicalEntity {
            canonical_id: canonical_id_for(e.kind, &e.text),
            kind: e.kind,
            surface: e.text.clone(),
            span_start: e.span_start,
            span_end: e.span_end,
            score: e.score,
        })
        .collect();

    // Promote topics. Dedup against the topic rows we already emitted so the
    // scorer producing the same label twice (LLM + regex overlap) collapses.
    // Entities under other kinds aren't dedup targets — `topic:launch` and
    // `hashtag:launch` are intentionally separate.
    for topic in &extracted.topics {
        let canonical_id = canonical_id_for(EntityKind::Topic, &topic.label);
        if out
            .iter()
            .any(|e| e.kind == EntityKind::Topic && e.canonical_id == canonical_id)
        {
            continue;
        }
        out.push(CanonicalEntity {
            canonical_id,
            kind: EntityKind::Topic,
            surface: topic.label.clone(),
            span_start: 0,
            span_end: 0,
            score: topic.score,
        });
    }
    out
}

/// Canonical id form per kind. Deterministic so the same surface always maps to
/// the same id.
///
/// - Email: `email:lowercased`
/// - Handle: `handle:lowercased` with leading `@` stripped
/// - Hashtag: `hashtag:lowercased` with leading `#` stripped
/// - URL: `url:trimmed` with case preserved for path/query exact matching
/// - Semantic kinds: `kind:lowercased-surface` (V1; fuzzy merge deferred)
pub fn canonical_id_for(kind: EntityKind, surface: &str) -> String {
    let trimmed = surface.trim();
    let clean = if kind == EntityKind::Url {
        trimmed.to_string()
    } else {
        trimmed
            .to_lowercase()
            .trim_start_matches('@')
            .trim_start_matches('#')
            .to_string()
    };
    format!("{}:{}", kind.as_str(), clean)
}

#[cfg(test)]
#[path = "resolver_tests.rs"]
mod tests;
