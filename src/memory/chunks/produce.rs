//! Markdown → bounded chunks with stable sequence numbers.
//!
//! The canonicalisers produce one big canonical Markdown blob per source
//! record; the chunker slices that into chunks of at most
//! [`DEFAULT_CHUNK_MAX_TOKENS`] so later phases can ingest them without blowing
//! past the summariser ceiling.
//!
//! ## Dispatch by source kind
//!
//! - **Chat**: split at `## ` message boundaries. Each message becomes one
//!   chunk. If a single message exceeds `max_tokens`, fall back to the
//!   paragraph/sentence/whitespace/char splitter for that unit only and emit
//!   each piece with `partial_message = true`.
//! - **Email**: split at `---\nFrom:` separators. Each email in the thread
//!   becomes one chunk. Same oversize fallback as Chat.
//! - **Document**: split by [`split_by_token_budget`] — a conservative
//!   token-estimate splitter (paragraph → sentence → whitespace → hard-char)
//!   with ~12% overlap between adjacent chunks.
//!
//! Chunk sizes are bounded by [`conservative_token_estimate`], not the GPT
//! `chars/4` heuristic, so dense markdown/hash/code/multilingual content cannot
//! produce an over-budget chunk that overflows a downstream embedder.

pub(crate) use super::produce_split::split_by_token_budget;
use super::types::{approx_token_count, chunk_id, Chunk, Metadata, SourceKind};

/// Default upper bound on per-chunk tokens. Well below the tree input budget so
/// each L0 seal accumulates many chunks before firing.
pub const DEFAULT_CHUNK_MAX_TOKENS: u32 = 3_000;

/// Tunable settings for the chunker.
#[derive(Clone, Debug)]
pub struct ChunkerOptions {
    /// Upper bound on per-chunk tokens.
    pub max_tokens: u32,
}

impl Default for ChunkerOptions {
    fn default() -> Self {
        Self {
            max_tokens: DEFAULT_CHUNK_MAX_TOKENS,
        }
    }
}

/// Input to the chunker: the canonicalised source and its provenance.
///
/// Callers own construction; the chunker does not interpret metadata beyond
/// cloning it onto each chunk.
#[derive(Clone, Debug)]
pub struct ChunkerInput {
    /// Source kind driving the splitting strategy.
    pub source_kind: SourceKind,
    /// Stable logical source id (used for deterministic chunk ids).
    pub source_id: String,
    /// Canonical Markdown content — possibly very long.
    pub markdown: String,
    /// Base metadata; cloned onto every produced chunk.
    pub metadata: Metadata,
}

/// Slice `input.markdown` into chunks ≤ `opts.max_tokens` tokens each.
///
/// Returns chunks in source order with stable sequence numbers starting at 0.
/// Chunk IDs are deterministic, so re-chunking yields the same ids for
/// identical input.
///
/// ## Dispatch by source kind
///
/// - **Chat / Email**: one chunk per message/email boundary. Oversize units
///   fall back to the bounded splitter and emit each piece with
///   `partial_message = true`. IDs derive from the complete logical unit plus
///   the piece index, so overlapping deliveries are stable regardless of batch
///   position.
/// - **Document**: split by the bounded token-budget helper — sized by the
///   conservative token estimate (paragraph → sentence → whitespace →
///   hard-char) with ~12% overlap between adjacent chunks.
///
pub fn chunk_markdown(input: &ChunkerInput, opts: &ChunkerOptions) -> Vec<Chunk> {
    let now = chrono::Utc::now();
    let max_tokens = opts.max_tokens.max(1);
    let max_chars = (max_tokens as usize).saturating_mul(4);

    // Dispatch: pick splitting units based on source kind.
    let units: Vec<String> = match input.source_kind {
        SourceKind::Chat => split_chat_messages(&input.markdown),
        SourceKind::Email => split_email_messages(&input.markdown),
        SourceKind::Document => split_by_token_budget(&input.markdown, max_tokens),
    };

    if matches!(input.source_kind, SourceKind::Document) {
        // Already split by budget; wrap directly.
        return units
            .into_iter()
            .enumerate()
            .map(|(idx, content)| {
                let seq = idx as u32;
                let token_count = approx_token_count(&content);
                let id = chunk_id(input.source_kind, &input.source_id, seq, &content);
                Chunk {
                    id,
                    content,
                    metadata: input.metadata.clone(),
                    token_count,
                    seq_in_source: seq,
                    created_at: now,
                    partial_message: false,
                }
            })
            .collect();
    }

    // Chat/email IDs are tied to each complete logical unit, not its batch-local
    // sequence. This makes overlapping redelivery idempotent.
    let mut out: Vec<Chunk> = Vec::new();
    for unit in units {
        let unit_chars = unit.chars().count();

        if unit_chars > max_chars {
            let sub_pieces = split_by_token_budget(&unit, max_tokens);
            for (part, piece) in sub_pieces.into_iter().enumerate() {
                let seq = out.len() as u32;
                let tc = approx_token_count(&piece);
                let id = chunk_id(input.source_kind, &input.source_id, part as u32, &unit);
                out.push(Chunk {
                    id,
                    content: piece,
                    metadata: input.metadata.clone(),
                    token_count: tc,
                    seq_in_source: seq,
                    created_at: now,
                    partial_message: true,
                });
            }
            continue;
        }
        let seq = out.len() as u32;
        let token_count = approx_token_count(&unit);
        let id = chunk_id(input.source_kind, &input.source_id, 0, &unit);
        out.push(Chunk {
            id,
            content: unit,
            metadata: input.metadata.clone(),
            token_count,
            seq_in_source: seq,
            created_at: now,
            partial_message: false,
        });
    }

    if out.is_empty() {
        // Degenerate: empty input → one empty chunk, matching original behaviour.
        let id = chunk_id(input.source_kind, &input.source_id, 0, "");
        out.push(Chunk {
            id,
            content: String::new(),
            metadata: input.metadata.clone(),
            token_count: 0,
            seq_in_source: 0,
            created_at: now,
            partial_message: false,
        });
    }

    out
}

/// Split a canonical chat blob into per-message units at `## ` boundaries.
///
/// Each returned string starts with `## ` and includes everything up to but
/// not including the next `## ` boundary. Lines before the first `## ` header
/// are dropped silently.
fn split_chat_messages(md: &str) -> Vec<String> {
    let mut pieces: Vec<String> = Vec::new();
    let mut current: Option<String> = None;

    for line in md.split_inclusive('\n') {
        if line.starts_with("## ") {
            if let Some(prev) = current.take() {
                let trimmed = prev.trim_end().to_string();
                if !trimmed.is_empty() {
                    pieces.push(trimmed);
                }
            }
            current = Some(line.to_string());
        } else if let Some(ref mut buf) = current {
            buf.push_str(line);
        }
        // Lines before the first `## ` (e.g. a leading `# ` header) are dropped.
    }

    if let Some(prev) = current.take() {
        let trimmed = prev.trim_end().to_string();
        if !trimmed.is_empty() {
            pieces.push(trimmed);
        }
    }

    if pieces.is_empty() && !md.trim().is_empty() {
        // No `## ` found at all — treat whole blob as one unit.
        pieces.push(md.trim_end().to_string());
    }

    pieces
}

/// Split a canonical email thread blob into per-email units.
///
/// Splits at `---` (alone on a line) followed by a `From:` line within the next
/// 8 lines. Each piece includes the `---` separator and everything up to but
/// not including the next `---\nFrom:` boundary. Content before the first
/// separator is dropped.
fn split_email_messages(md: &str) -> Vec<String> {
    let lines: Vec<&str> = md.split('\n').collect();
    let n = lines.len();
    let mut split_positions: Vec<usize> = Vec::new();

    for i in 0..n {
        let line = lines[i].trim_end();
        if line == "---" {
            // Check if one of the next 8 lines starts with `From:`
            let window_end = (i + 9).min(n);
            for candidate in lines.iter().take(window_end).skip(i + 1) {
                if candidate.starts_with("From:") {
                    split_positions.push(i);
                    break;
                }
                // Skip blank lines between `---` and `From:`
                if !candidate.trim().is_empty() {
                    break;
                }
            }
        }
    }

    if split_positions.is_empty() {
        // No email separator found — treat whole blob as one unit.
        let trimmed = md.trim_end().to_string();
        if trimmed.is_empty() {
            return Vec::new();
        }
        return vec![trimmed];
    }

    let mut pieces: Vec<String> = Vec::new();
    for (idx, &start) in split_positions.iter().enumerate() {
        let end = if idx + 1 < split_positions.len() {
            split_positions[idx + 1]
        } else {
            n
        };
        let piece_lines: Vec<&str> = lines[start..end].to_vec();
        let piece = piece_lines.join("\n").trim_end().to_string();
        if !piece.is_empty() {
            pieces.push(piece);
        }
    }

    pieces
}

#[cfg(test)]
#[path = "produce_tests.rs"]
mod tests;
