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
/// - **Chat / Email**: split at message/email boundaries, then greedy-pack
///   consecutive units into a single chunk until adding the next unit would
///   exceed `max_tokens`. Oversize units (a single message > `max_tokens`)
///   fall back to [`split_by_token_budget`] and emit each piece with
///   `partial_message = true`.
/// - **Document**: split by [`split_by_token_budget`] — sized by the
///   conservative token estimate (paragraph → sentence → whitespace →
///   hard-char) with ~12% overlap between adjacent chunks.
///
/// # NOTE — re-ingest of a grown source can leave stale rows (audit SC-8)
/// Chunk ids are deterministic in `(source_kind, source_id, seq, content)`, so
/// re-chunking byte-identical input reproduces the exact same ids — an
/// idempotent no-op against the store. But greedy packing means *appending*
/// new messages/emails to a source changes the content of what used to be the
/// last packed chunk at a given `seq`, so re-chunking a grown source produces
/// a **new** id at that `seq` rather than replacing the old row. This
/// function itself has no store access and cannot dedupe across calls; the
/// caller ([`super::store::upsert_chunks`]) only adds/replaces by id, so the
/// old row from the pre-growth chunking is never removed unless the caller
/// explicitly reconciles by source.
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

    // For Chat and Email: greedy-pack consecutive units into chunks.
    let unit_separator = "\n\n";
    let sep_chars = unit_separator.chars().count();

    let mut out: Vec<Chunk> = Vec::new();
    let mut acc: Vec<String> = Vec::new();
    let mut acc_chars = 0usize;

    // Flush accumulated units as one packed chunk.
    let flush = |acc: &mut Vec<String>, acc_chars: &mut usize, out: &mut Vec<Chunk>| {
        if acc.is_empty() {
            return;
        }
        let content = acc.join(unit_separator);
        let seq = out.len() as u32;
        let tc = approx_token_count(&content);
        let id = chunk_id(input.source_kind, &input.source_id, seq, &content);
        out.push(Chunk {
            id,
            content,
            metadata: input.metadata.clone(),
            token_count: tc,
            seq_in_source: seq,
            created_at: now,
            partial_message: false,
        });
        acc.clear();
        *acc_chars = 0;
    };

    for unit in units {
        let unit_chars = unit.chars().count();

        if unit_chars > max_chars {
            // Oversize: flush any pending accumulator first, then sub-split.
            flush(&mut acc, &mut acc_chars, &mut out);
            let sub_pieces = split_by_token_budget(&unit, max_tokens);
            for piece in sub_pieces {
                let seq = out.len() as u32;
                let tc = approx_token_count(&piece);
                let id = chunk_id(input.source_kind, &input.source_id, seq, &piece);
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

        // Compute projected size if we add this unit to the accumulator.
        let projected = if acc.is_empty() {
            unit_chars
        } else {
            acc_chars + sep_chars + unit_chars
        };

        if projected > max_chars {
            // Adding this unit would overflow — flush the accumulator first.
            flush(&mut acc, &mut acc_chars, &mut out);
        }

        if !acc.is_empty() {
            acc_chars += sep_chars;
        }
        acc_chars += unit_chars;
        acc.push(unit);
    }

    // Flush any remaining accumulated units.
    flush(&mut acc, &mut acc_chars, &mut out);

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
            for j in (i + 1)..window_end {
                if lines[j].starts_with("From:") {
                    split_positions.push(i);
                    break;
                }
                // Skip blank lines between `---` and `From:`
                if !lines[j].trim().is_empty() {
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
