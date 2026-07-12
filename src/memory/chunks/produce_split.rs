//! The conservative-token budget splitter used by [`super::produce`].
//!
//! Splits arbitrary text into pieces each within a conservative token budget,
//! preferring paragraph → sentence → whitespace → hard-char boundaries, with a
//! small overlap carried between adjacent pieces so a fact straddling a split
//! survives in both neighbours.

use super::types::{conservative_token_estimate, truncate_to_conservative_tokens};

/// Overlap carried between adjacent chunks (≈12%) so a fact straddling a split
/// boundary survives in both neighbours.
const OVERLAP_PERCENT: u32 = 12;
/// Hard cap on overlap (% of budget) so a chunk can never be mostly a duplicate
/// of its predecessor.
const OVERLAP_MAX_PERCENT: u32 = 40;

/// Split `text` into pieces each ≤ `max_tokens` **conservative** tokens
/// (see [`conservative_token_estimate`]), with ~10–15% overlap between adjacent
/// pieces.
///
/// Boundary preference (a still-oversized piece falls to the next finer level):
/// 1. paragraph (`\n\n`)
/// 2. sentence (`. ` / `! ` / `? ` / line break)
/// 3. whitespace (word)
/// 4. hard character cut (last resort; preserves UTF-8 code points)
///
/// Ordering is preserved. Overlap repeats the previous piece's trailing whole
/// segments verbatim (snapped to natural boundaries), never the entire chunk.
pub(crate) fn split_by_token_budget(text: &str, max_tokens: u32) -> Vec<String> {
    let budget = max_tokens.max(1);
    if text.is_empty() {
        return vec![String::new()];
    }
    if conservative_token_estimate(text) <= budget {
        return vec![text.to_string()];
    }

    // Reserve headroom so `overlap (≤cap) + any segment (≤seg_budget) ≤ budget`.
    let overlap_budget = (budget * OVERLAP_PERCENT / 100).max(1);
    let overlap_cap = (budget * OVERLAP_MAX_PERCENT / 100).max(overlap_budget);
    let seg_budget = budget.saturating_sub(overlap_cap).max(1);

    // 1. Reduce to in-order atomic segments, each ≤ seg_budget.
    let mut segments: Vec<&str> = Vec::new();
    push_atomic_segments(text, seg_budget, &mut segments);
    if segments.len() <= 1 {
        return vec![text.to_string()];
    }

    // 2. Greedy-pack segments into chunks ≤ budget, carrying overlap forward.
    let mut chunks: Vec<String> = Vec::new();
    let mut cur: Vec<&str> = Vec::new();
    let mut cur_tokens = 0u32;
    for seg in &segments {
        let seg_tokens = conservative_token_estimate(seg);
        if !cur.is_empty() && cur_tokens.saturating_add(seg_tokens) > budget {
            chunks.push(join_segments(&cur));
            let overlap = tail_overlap(&cur, overlap_budget, overlap_cap);
            cur_tokens = overlap.iter().map(|s| conservative_token_estimate(s)).sum();
            cur = overlap;
        }
        cur.push(seg);
        cur_tokens = cur_tokens.saturating_add(seg_tokens);
    }
    if !cur.is_empty() {
        chunks.push(join_segments(&cur));
    }
    if chunks.is_empty() {
        chunks.push(String::new());
    }
    chunks
}

/// Append in-order atomic segments of `text`, each with
/// `conservative_token_estimate ≤ budget`, using the boundary hierarchy.
fn push_atomic_segments<'a>(text: &'a str, budget: u32, out: &mut Vec<&'a str>) {
    if text.is_empty() {
        return;
    }
    if conservative_token_estimate(text) <= budget {
        out.push(text);
        return;
    }
    if split_on_separator(text, "\n\n", budget, out)
        || split_on_sentences(text, budget, out)
        || split_on_whitespace(text, budget, out)
    {
        return;
    }
    hard_split(text, budget, out);
}

/// Split on a literal separator; recurse on each piece. The separator is kept
/// at the END of its preceding piece so the pieces tile `text` exactly. Returns
/// `false` (no progress) when the separator does not occur.
fn split_on_separator<'a>(text: &'a str, sep: &str, budget: u32, out: &mut Vec<&'a str>) -> bool {
    if sep.is_empty() || !text.contains(sep) {
        return false;
    }
    let sep_len = sep.len();
    let mut pieces: Vec<&str> = Vec::new();
    let mut start = 0usize;
    let mut search = 0usize;
    while let Some(rel) = text[search..].find(sep) {
        let end = search + rel + sep_len; // include the separator in this piece
        pieces.push(&text[start..end]);
        start = end;
        search = end;
    }
    if start < text.len() {
        pieces.push(&text[start..]);
    }
    if pieces.len() <= 1 {
        return false;
    }
    for p in pieces {
        push_atomic_segments(p, budget, out);
    }
    true
}

/// Split on sentence-ish boundaries: after `.`/`!`/`?` followed by a space, and
/// at line breaks. All boundary bytes are ASCII, so slicing is always on a char
/// boundary. Returns `false` when no boundary is found.
fn split_on_sentences<'a>(text: &'a str, budget: u32, out: &mut Vec<&'a str>) -> bool {
    let bytes = text.as_bytes();
    let mut pieces: Vec<&str> = Vec::new();
    let mut start = 0usize;
    for i in 0..bytes.len() {
        let c = bytes[i];
        let sentence_end =
            matches!(c, b'.' | b'!' | b'?') && i + 1 < bytes.len() && bytes[i + 1] == b' ';
        let boundary_end = if c == b'\n' || sentence_end {
            Some(i + 1)
        } else {
            None
        };
        if let Some(end) = boundary_end {
            pieces.push(&text[start..end]);
            start = end;
        }
    }
    if start < text.len() {
        pieces.push(&text[start..]);
    }
    if pieces.len() <= 1 {
        return false;
    }
    for p in pieces {
        push_atomic_segments(p, budget, out);
    }
    true
}

/// Split on ASCII spaces (word boundaries); recurse on each word. Returns
/// `false` when there is no space to split on.
fn split_on_whitespace<'a>(text: &'a str, budget: u32, out: &mut Vec<&'a str>) -> bool {
    let bytes = text.as_bytes();
    let mut pieces: Vec<&str> = Vec::new();
    let mut start = 0usize;
    for i in 0..bytes.len() {
        if bytes[i] == b' ' {
            // Keep the space at the end of the word so pieces tile `text` exactly.
            pieces.push(&text[start..=i]);
            start = i + 1;
        }
    }
    if start < text.len() {
        pieces.push(&text[start..]);
    }
    if pieces.len() <= 1 {
        return false;
    }
    for p in pieces {
        push_atomic_segments(p, budget, out);
    }
    true
}

/// Last resort: cut a boundary-free run into ≤ `budget`-token pieces on UTF-8
/// char boundaries. Reuses [`truncate_to_conservative_tokens`] for sizing and
/// always makes progress (≥1 char) so it cannot loop.
fn hard_split<'a>(mut text: &'a str, budget: u32, out: &mut Vec<&'a str>) {
    while !text.is_empty() {
        let mut piece = truncate_to_conservative_tokens(text, budget);
        if piece.is_empty() {
            // Budget smaller than one char's weight — take a single char.
            let end = text
                .char_indices()
                .nth(1)
                .map(|(i, _)| i)
                .unwrap_or(text.len());
            piece = &text[..end];
        }
        let plen = piece.len();
        out.push(&text[..plen]);
        text = &text[plen..];
    }
}

/// Join packed segments back into one chunk body. Segments are contiguous,
/// separator-retaining slices of the source, so plain concatenation reproduces
/// the original text exactly (modulo intentional overlap duplication).
fn join_segments(segs: &[&str]) -> String {
    segs.concat()
}

/// Trailing whole segments of `cur` summing to ~`overlap_budget` tokens (capped
/// at `overlap_cap`), returned in original order. Never returns the entire
/// chunk, so adjacent chunks cannot be duplicates.
fn tail_overlap<'a>(cur: &[&'a str], overlap_budget: u32, overlap_cap: u32) -> Vec<&'a str> {
    if cur.len() <= 1 {
        return Vec::new();
    }
    let mut acc = 0u32;
    let mut take = 0usize;
    for seg in cur.iter().rev() {
        let t = conservative_token_estimate(seg);
        // Cap EVERY trailing segment so `overlap <= overlap_cap`.
        if acc.saturating_add(t) > overlap_cap {
            break;
        }
        acc = acc.saturating_add(t);
        take += 1;
        if acc >= overlap_budget {
            break;
        }
    }
    if take >= cur.len() {
        take = cur.len() - 1; // never repeat the whole chunk
    }
    cur[cur.len() - take..].to_vec()
}
