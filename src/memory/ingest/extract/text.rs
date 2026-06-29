//! Text-normalisation helpers shared by the deterministic extractor.
//!
//! These are free-function ports of the `UnifiedMemory` static helpers used by
//! the OpenHuman ingestion pipeline. They depend only on the already-ported
//! semantic chunker (`crate::memory::chunks::chunk_semantic`).

use crate::memory::chunks::chunk_semantic;

/// Split document content into trimmed, non-empty chunk strings using the
/// semantic (heading/paragraph-aware) chunker. Falls back to the whole trimmed
/// content when the chunker yields nothing for non-empty input.
pub(super) fn chunk_document_content(content: &str, max_tokens: usize) -> Vec<String> {
    let mut chunks: Vec<String> = chunk_semantic(content, max_tokens.max(1))
        .into_iter()
        .map(|chunk| chunk.content.trim().to_string())
        .filter(|chunk: &String| !chunk.is_empty())
        .collect();
    if chunks.is_empty() && !content.trim().is_empty() {
        chunks.push(content.trim().to_string());
    }
    chunks
}

/// Collapse all runs of whitespace into single spaces and trim the edges.
pub(super) fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Lower-case, alphanumeric-preserving normalisation for substring search.
/// Separators (`_ - / .` and whitespace) collapse to single spaces.
pub(super) fn normalize_search_text(text: &str) -> String {
    let collapsed = collapse_whitespace(text);
    let mut normalized = String::with_capacity(collapsed.len());
    for ch in collapsed.chars() {
        if ch.is_alphanumeric() {
            normalized.extend(ch.to_lowercase());
        } else if ch.is_whitespace() || matches!(ch, '_' | '-' | '/' | '.') {
            normalized.push(' ');
        }
    }
    normalized.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Normalise a predicate to uppercase snake_case (e.g. `works on` → `WORKS_ON`).
pub(super) fn normalize_graph_predicate(text: &str) -> String {
    let mut out = String::new();
    let mut last_was_sep = false;
    for ch in collapse_whitespace(text.trim()).chars() {
        if ch.is_alphanumeric() {
            out.extend(ch.to_uppercase());
            last_was_sep = false;
        } else if !last_was_sep {
            out.push('_');
            last_was_sep = true;
        }
    }
    out.trim_matches('_').to_string()
}

#[cfg(test)]
#[path = "text_tests.rs"]
mod tests;
