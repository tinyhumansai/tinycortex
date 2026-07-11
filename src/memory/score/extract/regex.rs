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
///
/// Emits one [`ExtractedEntity`] per regex match (multiple matches of the
/// same surface form are **not** deduplicated here — that's
/// [`ExtractedEntities::merge`]'s job when combining extractor outputs).
/// Hashtag matches are additionally surfaced as [`ExtractedTopic`] rows so
/// they can be promoted into the canonical entity stream by the resolver.
///
/// NOTE: `RE_HANDLE` allows `.`/`-` anywhere in the captured group, including
/// at the end, so trailing sentence punctuation folded into a handle (e.g.
/// "ping @alice." at end-of-sentence) is captured as part of the surface
/// form. `handle:alice.` and `handle:alice` then canonicalise to distinct
/// entities, splitting co-occurrence weight for what is really one person
/// (see audit finding RS-12).
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

/// Build an [`ExtractedEntity`] for a regex match, converting the match's
/// byte offsets (`start`/`end`, as returned by the `regex` crate) into the
/// char offsets [`ExtractedEntity`] stores. Score is always `1.0` — regex
/// matches are deterministic and unambiguous.
fn to_entity(text: &str, start: usize, end: usize, kind: EntityKind) -> ExtractedEntity {
    ExtractedEntity {
        kind,
        text: text[start..end].to_string(),
        span_start: char_index(text, start),
        span_end: char_index(text, end),
        score: 1.0,
    }
}

/// Convert a byte offset into `s` to the corresponding char (Unicode
/// scalar value) offset, by counting chars in the prefix `s[..byte_idx]`.
/// `byte_idx` is clamped to `s.len()` first, so an out-of-range offset
/// degrades to "end of string" instead of panicking on an out-of-bounds
/// slice. Caller must still pass a byte index that lands on a UTF-8 char
/// boundary (true for all offsets the `regex` crate returns).
fn char_index(s: &str, byte_idx: usize) -> u32 {
    let byte_idx = byte_idx.min(s.len());
    s[..byte_idx].chars().count() as u32
}

#[cfg(test)]
#[path = "regex_tests.rs"]
mod tests;
