//! Semantic markdown chunking.
//!
//! This module provides the logic for splitting large markdown documents into
//! smaller, semantically meaningful chunks that fit within the context window
//! of an LLM or an embedding model. It prioritizes splitting on headings and
//! paragraph boundaries while preserving context by carrying over headings
//! to subsequent chunks.
//!
//! This is a distinct chunker from [`super::produce`]: [`chunk_markdown`]
//! (this module, re-exported as `chunk_semantic`) targets pre-chunking a
//! *single* large document by heading/paragraph structure for
//! LLM-context-sized pieces, using the simpler `chars/4` token heuristic
//! throughout (no [`super::types::conservative_token_estimate`] safety
//! margin). [`super::produce::chunk_markdown`] is the source-kind-aware
//! ingest-time chunker that produces the persisted [`super::types::Chunk`].
//! Neither one calls the other; callers pick whichever fits their use case.

use std::rc::Rc;

/// A single chunk of text extracted from a larger document.
#[derive(Debug, Clone)]
pub struct Chunk {
    /// The zero-based index of this chunk within the original document.
    pub index: usize,
    /// The actual text content of the chunk.
    pub content: String,
    /// The most recent markdown heading that applies to this chunk's content.
    /// Uses `Rc<str>` for efficient sharing of the same heading across multiple chunks.
    pub heading: Option<Rc<str>>,
}

/// Splits markdown text into a sequence of [`Chunk`] objects.
///
/// Each chunk is designed to be approximately under the `max_tokens` limit.
/// The chunker uses a hierarchical splitting strategy:
/// 1. **Heading Boundaries**: Splits on `#` through `######` headings.
/// 2. **Paragraph Boundaries**: If a heading section is too large, it splits on blank lines.
/// 3. **Line Boundaries**: If a paragraph is still too large, it splits on individual lines.
///
/// # Arguments
/// * `text` - The raw markdown text to chunk.
/// * `max_tokens` - The approximate maximum number of tokens per chunk (estimated at 4 chars/token).
///
/// # Returns
/// A vector of [`Chunk`] structs representing the document.
pub fn chunk_markdown(text: &str, max_tokens: usize) -> Vec<Chunk> {
    if text.trim().is_empty() {
        return Vec::new();
    }

    // Rough estimation: 4 characters per token for English text.
    let max_chars = max_tokens * 4;

    // Step 1: Divide the document into top-level sections based on headings.
    let sections = split_on_headings(text);
    let mut chunks = Vec::with_capacity(sections.len());

    for (heading, body) in sections {
        let heading: Option<Rc<str>> = heading.map(Rc::from);
        let heading_prefix = heading.as_deref().map(|h| {
            let mut prefix = String::with_capacity(h.len() + 1);
            prefix.push_str(h);
            prefix.push('\n');
            prefix
        });

        let full_len = body.len() + heading_prefix.as_ref().map_or(0, String::len);

        if full_len <= max_chars {
            // Section fits entirely in one chunk.
            let content = if let Some(prefix) = heading_prefix.as_deref() {
                let mut full = String::with_capacity(full_len);
                full.push_str(prefix);
                full.push_str(&body);
                full.trim().to_string()
            } else {
                body.trim().to_string()
            };
            chunks.push(Chunk {
                index: chunks.len(),
                content,
                heading: heading.clone(),
            });
        } else {
            // Step 2: Section is too large; split into paragraphs.
            let paragraphs = split_on_blank_lines(&body);
            let mut current = heading_prefix.clone().unwrap_or_default();

            for para in paragraphs {
                // If adding this paragraph exceeds the limit, emit the current chunk.
                if current.len() + para.len() > max_chars && !current.trim().is_empty() {
                    chunks.push(Chunk {
                        index: chunks.len(),
                        content: current.trim().to_string(),
                        heading: heading.clone(),
                    });
                    // Reset with the heading for context preservation.
                    reset_chunk_buffer(&mut current, heading_prefix.as_deref());
                }

                if para.len() > max_chars {
                    // Step 3: Paragraph is still too large; split it line-by-line.
                    if !current.trim().is_empty() {
                        chunks.push(Chunk {
                            index: chunks.len(),
                            content: current.trim().to_string(),
                            heading: heading.clone(),
                        });
                        reset_chunk_buffer(&mut current, heading_prefix.as_deref());
                    }
                    for line_chunk in split_on_lines(&para, max_chars) {
                        chunks.push(Chunk {
                            index: chunks.len(),
                            content: line_chunk.trim().to_string(),
                            heading: heading.clone(),
                        });
                    }
                } else {
                    current.push_str(&para);
                    current.push('\n');
                }
            }

            // Emit any remaining content as a final chunk for this section.
            if !current.trim().is_empty() {
                chunks.push(Chunk {
                    index: chunks.len(),
                    content: current.trim().to_string(),
                    heading: heading.clone(),
                });
            }
        }
    }

    // Clean up empty chunks and normalize indices.
    chunks.retain(|c| !c.content.is_empty());

    for (i, chunk) in chunks.iter_mut().enumerate() {
        chunk.index = i;
    }

    chunks
}

/// Clear `current` and, if this section has a heading, re-seed the buffer
/// with `heading_prefix` so the next emitted chunk still carries heading
/// context even though it starts a fresh paragraph run.
fn reset_chunk_buffer(current: &mut String, heading_prefix: Option<&str>) {
    current.clear();
    if let Some(prefix) = heading_prefix {
        current.push_str(prefix);
    }
}

/// Returns `true` if `line` starts with a valid ATX markdown heading
/// (1 to 6 `#` characters followed by a space).
fn is_atx_heading(line: &str) -> bool {
    const PREFIXES: &[&str] = &["# ", "## ", "### ", "#### ", "##### ", "###### "];
    PREFIXES.iter().any(|p| line.starts_with(p))
}

/// Identifies markdown ATX headings and groups their following text into
/// sections.
fn split_on_headings(text: &str) -> Vec<(Option<String>, String)> {
    let mut sections = Vec::new();
    let mut current_heading: Option<String> = None;
    let mut current_body = String::new();

    for line in text.lines() {
        if is_atx_heading(line) {
            if !current_body.trim().is_empty() || current_heading.is_some() {
                sections.push((current_heading.take(), std::mem::take(&mut current_body)));
            }
            current_heading = Some(line.to_string());
        } else {
            current_body.push_str(line);
            current_body.push('\n');
        }
    }

    if !current_body.trim().is_empty() || current_heading.is_some() {
        sections.push((current_heading, current_body));
    }

    sections
}

/// Splits text into strings based on blank line (paragraph) boundaries.
fn split_on_blank_lines(text: &str) -> Vec<String> {
    let mut paragraphs = Vec::new();
    let mut current = String::new();

    for line in text.lines() {
        if line.trim().is_empty() {
            if !current.trim().is_empty() {
                paragraphs.push(std::mem::take(&mut current));
            }
        } else {
            current.push_str(line);
            current.push('\n');
        }
    }

    if !current.trim().is_empty() {
        paragraphs.push(current);
    }

    paragraphs
}

/// Splits text into chunks based on line boundaries to ensure size constraints.
/// Lines exceeding `max_chars` are further split on word boundaries.
fn split_on_lines(text: &str, max_chars: usize) -> Vec<String> {
    let effective_max = max_chars.max(1);
    let mut chunks = Vec::with_capacity(text.len() / effective_max + 1);
    let mut current = String::new();

    for line in text.lines() {
        if line.len() > effective_max {
            // Flush anything accumulated before the oversize line.
            if !current.is_empty() {
                chunks.push(std::mem::take(&mut current));
            }
            // Split the oversize line itself on word boundaries.
            for part in split_within_line(line, effective_max) {
                chunks.push(part);
            }
        } else if current.len() + line.len() + 1 > effective_max && !current.is_empty() {
            chunks.push(std::mem::take(&mut current));
            current.push_str(line);
            current.push('\n');
        } else {
            current.push_str(line);
            current.push('\n');
        }
    }

    if !current.is_empty() {
        chunks.push(current);
    }

    chunks
}

/// Splits a single oversize line into chunks of at most `max_chars`, preferring
/// word boundaries (spaces) to avoid cutting mid-word. Falls back to hard
/// character splits when no boundary exists within the limit.
fn split_within_line(line: &str, max_chars: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut start = 0;
    let bytes = line.as_bytes();

    while start < line.len() {
        let remaining = line.len() - start;
        if remaining <= max_chars {
            chunks.push(format!("{}\n", &line[start..]));
            break;
        }

        // Find the end boundary, staying on a valid char boundary.
        let mut end = start + max_chars;
        while end > start && !line.is_char_boundary(end) {
            end -= 1;
        }

        // If max_chars is smaller than the next character, `end` can equal
        // `start`. Advance to the next char boundary to guarantee progress.
        if end == start {
            end = start + 1;
            while end < line.len() && !line.is_char_boundary(end) {
                end += 1;
            }
        }

        // Try to find a space to break on (scan backwards from `end`).
        let mut split_at = end;
        while split_at > start && bytes[split_at - 1] != b' ' {
            split_at -= 1;
        }

        // If we couldn't find a space within the range, hard-split at `end`.
        if split_at == start {
            split_at = end;
        }

        chunks.push(format!("{}\n", &line[start..split_at]));
        // Skip the space we split on (if it was a space).
        if split_at < line.len() && bytes[split_at] == b' ' {
            start = split_at + 1;
        } else {
            start = split_at;
        }
    }

    chunks
}

#[cfg(test)]
#[path = "semantic_tests.rs"]
mod tests;
