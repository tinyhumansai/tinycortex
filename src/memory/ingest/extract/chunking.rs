//! Chunking helpers: sentence splitting, extraction-unit grouping, and the
//! excerpt → chunk-index search used to attribute extracted facts to chunks.

use super::regex::sanitize_fact_text;
use super::text::normalize_search_text;
use super::types::{ExtractionMode, ExtractionUnit};

/// Splits a document into individual sentences based on punctuation and line
/// breaks.
pub(super) fn split_sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        current.push(ch);
        if matches!(ch, '.' | '!' | '?' | '\n') {
            let candidate = sanitize_fact_text(&current);
            if !candidate.is_empty() {
                out.push(candidate);
            }
            current.clear();
        }
    }
    let tail = sanitize_fact_text(&current);
    if !tail.is_empty() {
        out.push(tail);
    }
    let mut merged: Vec<String> = Vec::new();
    for sentence in out {
        if sentence.len() < 5 && !merged.is_empty() {
            if let Some(last) = merged.last_mut() {
                last.push(' ');
                last.push_str(&sentence);
            }
        } else {
            merged.push(sentence);
        }
    }
    if merged.is_empty() && !text.trim().is_empty() {
        merged.push(sanitize_fact_text(text));
    }
    merged
}

/// Groups chunks into extraction units based on the configured mode.
pub(super) fn build_units(chunks: &[String], mode: ExtractionMode) -> Vec<ExtractionUnit> {
    let mut units = Vec::new();
    let mut order_index = 0_i64;
    for (chunk_index, chunk) in chunks.iter().enumerate() {
        match mode {
            ExtractionMode::Chunk => {
                let text = sanitize_fact_text(chunk);
                if text.is_empty() {
                    continue;
                }
                units.push(ExtractionUnit {
                    text,
                    chunk_index,
                    order_index,
                });
                order_index += 1;
            }
            ExtractionMode::Sentence => {
                for sentence in split_sentences(chunk) {
                    if sentence.is_empty() {
                        continue;
                    }
                    units.push(ExtractionUnit {
                        text: sentence,
                        chunk_index,
                        order_index,
                    });
                    order_index += 1;
                }
            }
        }
    }
    units
}

/// Searches for the chunk index that most likely contains the given excerpt.
pub(super) fn find_chunk_index(chunks: &[String], excerpt: &str, hint: usize) -> usize {
    if chunks.is_empty() {
        return 0;
    }
    let needle = normalize_search_text(excerpt);
    if needle.is_empty() {
        return hint.min(chunks.len().saturating_sub(1));
    }
    for (index, chunk) in chunks.iter().enumerate().skip(hint) {
        if normalize_search_text(chunk).contains(&needle) {
            return index;
        }
    }
    for (index, chunk) in chunks.iter().enumerate().take(hint.min(chunks.len())) {
        if normalize_search_text(chunk).contains(&needle) {
            return index;
        }
    }
    hint.min(chunks.len().saturating_sub(1))
}
