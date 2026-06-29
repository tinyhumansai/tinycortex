//! Deterministic mechanical-entity extraction via regex.
//!
//! Catches the shapes regex handles cleanly and that are genuinely useful
//! as cross-platform identity anchors (email appearing in Slack + Gmail =
//! same person):
//!
//! - **Email** — RFC-ish pattern, boundary-guarded
//! - **URL** — `http(s)://…` up to whitespace or trailing punctuation
//! - **Handle** — `@alice`, `@alice.bsky.social`, or `alice#1234`
//! - **Hashtag** — `#launch-q2`
//!
//! Every match has `score = 1.0` (regex is deterministic). Spans are
//! char-offsets (not bytes) for UTF-8 safety.

use std::sync::LazyLock;

use regex::Regex;

use super::types::{EntityKind, ExtractedEntities, ExtractedEntity, ExtractedTopic};

// ── Compiled regexes (once per process) ──────────────────────────────────

static RE_EMAIL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b[a-z0-9._%+\-]+@[a-z0-9.\-]+\.[a-z]{2,}\b").unwrap());

static RE_URL: LazyLock<Regex> = LazyLock::new(|| {
    // up-to trailing punctuation; avoids catastrophic backtracking
    Regex::new(r"https?://[^\s<>\]\[()]+[^\s<>\]\[()\.\,;:\!\?]").unwrap()
});

static RE_HANDLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|[\s(])@([A-Za-z0-9_][A-Za-z0-9_.\-]{1,})").unwrap());

static RE_DISCRIM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b([A-Za-z0-9_.\-]{2,32})#\d{4}\b").unwrap());

static RE_HASHTAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|[\s(])#([A-Za-z][A-Za-z0-9_\-]{1,})").unwrap());

/// Extract all mechanical entities from `text`.
pub fn extract(text: &str) -> ExtractedEntities {
    let mut entities: Vec<ExtractedEntity> = Vec::new();
    let mut topics: Vec<ExtractedTopic> = Vec::new();

    for m in RE_EMAIL.find_iter(text) {
        entities.push(to_entity(text, m.start(), m.end(), EntityKind::Email));
    }
    for m in RE_URL.find_iter(text) {
        entities.push(to_entity(text, m.start(), m.end(), EntityKind::Url));
    }
    for cap in RE_HANDLE.captures_iter(text) {
        if let Some(m) = cap.get(1) {
            entities.push(to_entity(text, m.start(), m.end(), EntityKind::Handle));
        }
    }
    for cap in RE_DISCRIM.captures_iter(text) {
        if let Some(m) = cap.get(0) {
            entities.push(to_entity(text, m.start(), m.end(), EntityKind::Handle));
        }
    }
    for cap in RE_HASHTAG.captures_iter(text) {
        if let Some(m) = cap.get(1) {
            entities.push(to_entity(text, m.start(), m.end(), EntityKind::Hashtag));
            topics.push(ExtractedTopic {
                label: text[m.start()..m.end()].to_lowercase(),
                score: 1.0,
            });
        }
    }

    ExtractedEntities {
        entities,
        topics,
        // Regex extractor never produces an LLM importance signal.
        llm_importance: None,
        llm_importance_reason: None,
    }
}

fn to_entity(text: &str, start: usize, end: usize, kind: EntityKind) -> ExtractedEntity {
    ExtractedEntity {
        kind,
        text: text[start..end].to_string(),
        span_start: char_index(text, start),
        span_end: char_index(text, end),
        score: 1.0,
    }
}

fn char_index(s: &str, byte_idx: usize) -> u32 {
    let byte_idx = byte_idx.min(s.len());
    s[..byte_idx].chars().count() as u32
}

#[cfg(test)]
#[path = "regex_tests.rs"]
mod tests;
