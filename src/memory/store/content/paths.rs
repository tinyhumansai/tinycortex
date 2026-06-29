//! Content-file path generation.
//!
//! Each chunk body is stored as a `.md` file under `<content_root>/`. The path
//! structure depends on the source kind:
//!
//! ```text
//! Email:    <content_root>/email/<participants_slug>/<chunk_id>.md
//! Chat:     <content_root>/chat/<source_slug>/<chunk_id>.md
//! Document: <content_root>/document/<source_slug>/<chunk_id>.md
//! ```
//!
//! Paths are stored in SQLite as **relative** strings with forward slashes so
//! they remain valid regardless of where the workspace is mounted.

use std::path::{Path, PathBuf};

/// Which kind of summary tree a summary belongs to. Determines the folder name
/// under `<content_root>/wiki/summaries/`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SummaryTreeKind {
    /// Per-source-tree summary. Layout: `wiki/summaries/source-<scope_slug>/L<level>/<id>.md`
    Source,
    /// Global digest tree — the singleton cross-source activity tree. Layout:
    /// `wiki/summaries/global/L<level>/<id>.md`.
    Global,
    /// Per-topic (entity) tree. Layout: `wiki/summaries/topic-<scope_slug>/L<level>/<id>.md`
    Topic,
}

/// Top-level directory for derived/wiki content (summaries, etc.).
pub const WIKI_PREFIX: &str = "wiki";

/// Build the relative content path for a summary, using forward slashes.
///
/// `scope_slug` must already be slugified by the caller (use [`slugify_source_id`]).
/// A trailing `.md` on `summary_id` is stripped if present.
pub fn summary_rel_path(
    tree_kind: SummaryTreeKind,
    scope_slug: &str,
    level: u32,
    summary_id: &str,
) -> String {
    let filename = summary_filename(summary_id);

    match tree_kind {
        SummaryTreeKind::Source => {
            format!("{WIKI_PREFIX}/summaries/source-{scope_slug}/L{level}/{filename}.md")
        }
        SummaryTreeKind::Global => {
            format!("{WIKI_PREFIX}/summaries/global/L{level}/{filename}.md")
        }
        SummaryTreeKind::Topic => {
            format!("{WIKI_PREFIX}/summaries/topic-{scope_slug}/L{level}/{filename}.md")
        }
    }
}

/// On-disk placement for a summary node within a document **source** tree.
#[derive(Clone, Copy, Debug)]
pub enum SummaryDiskLayout<'a> {
    /// Flat layout — `source-<scope>/L<level>/…` (chat, email, legacy).
    Standard,
    /// A node inside one document's versioned subtree —
    /// `source-<scope>/docs/<doc_slug>/v-<version_ms>/L<level>/…`.
    DocSubtree {
        doc_slug: &'a str,
        version_ms: Option<i64>,
    },
    /// A cross-document merge-tier node — `source-<scope>/merge/L<level>/…`.
    Merge,
}

/// Layout-aware variant of [`summary_rel_path`]. For document source trees it
/// routes per-doc and merge nodes into nested folders; for everything else it
/// is identical to [`summary_rel_path`].
pub fn summary_rel_path_with_layout(
    tree_kind: SummaryTreeKind,
    scope_slug: &str,
    level: u32,
    summary_id: &str,
    layout: SummaryDiskLayout<'_>,
) -> String {
    match (tree_kind, layout) {
        (
            SummaryTreeKind::Source,
            SummaryDiskLayout::DocSubtree {
                doc_slug,
                version_ms,
            },
        ) => {
            let filename = summary_filename(summary_id);
            let safe_slug = slugify_source_id(doc_slug);
            let vfolder = match version_ms {
                Some(v) => format!("v-{v}"),
                None => "v-unversioned".to_string(),
            };
            format!(
                "{WIKI_PREFIX}/summaries/source-{scope_slug}/docs/{safe_slug}/{vfolder}/L{level}/{filename}.md"
            )
        }
        (SummaryTreeKind::Source, SummaryDiskLayout::Merge) => {
            let filename = summary_filename(summary_id);
            format!("{WIKI_PREFIX}/summaries/source-{scope_slug}/merge/L{level}/{filename}.md")
        }
        _ => summary_rel_path(tree_kind, scope_slug, level, summary_id),
    }
}

/// Convert a summary id into the canonical on-disk basename stem (without `.md`).
pub(crate) fn summary_filename(summary_id: &str) -> String {
    let id = summary_id.strip_suffix(".md").unwrap_or(summary_id);

    if let Some(rest) = id.strip_prefix("summary:") {
        if let Some((ms, suffix)) = rest.split_once(':') {
            if let Some((level, tail)) = suffix.split_once('-') {
                let level_is_numeric = level.starts_with('L')
                    && level.len() > 1
                    && level[1..].chars().all(|c| c.is_ascii_digit());
                let tail_is_safe = !tail.is_empty()
                    && !tail
                        .chars()
                        .any(|c| matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|'));
                if ms.len() == 13
                    && ms.chars().all(|c| c.is_ascii_digit())
                    && level_is_numeric
                    && tail_is_safe
                {
                    return format!("summary-{ms}-{level}-{tail}");
                }
            }
        }

        if let Some((level, tail)) = rest.split_once(':') {
            let level_is_numeric = level.starts_with('L')
                && level.len() > 1
                && level[1..].chars().all(|c| c.is_ascii_digit());
            if level_is_numeric && !tail.is_empty() {
                return format!("summary-{level}-{}", sanitize_filename(tail));
            }
        }
    }

    sanitize_filename(id)
}

/// Replace characters that are illegal in filenames on Windows NTFS with `-`.
pub(crate) fn sanitize_filename(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            c => c,
        })
        .collect()
}

/// Build the absolute on-disk path for a summary given the content root.
pub fn summary_abs_path(
    content_root: &Path,
    tree_kind: SummaryTreeKind,
    scope_slug: &str,
    level: u32,
    summary_id: &str,
) -> PathBuf {
    let rel = summary_rel_path(tree_kind, scope_slug, level, summary_id);
    let mut abs = content_root.to_path_buf();
    for component in rel.split('/') {
        abs.push(component);
    }
    abs
}

/// Build the relative content path for a chunk, using forward slashes.
///
/// `chunk_id` — the deterministic content hash produced by `chunk_id`.
pub fn chunk_rel_path(source_kind: &str, source_id: &str, chunk_id: &str) -> String {
    let filename = sanitize_filename(chunk_id);
    match source_kind {
        "email" => {
            let parts: Vec<&str> = source_id.splitn(2, ':').collect();
            if parts.len() == 2 && parts[0] == "gmail" && !parts[1].is_empty() {
                let participants_slug = slugify_source_id(parts[1]);
                format!("email/{participants_slug}/{filename}.md")
            } else {
                let slug = slugify_source_id(source_id);
                format!("email/{slug}/{filename}.md")
            }
        }
        _ => {
            let slug = slugify_source_id(source_id);
            format!("{source_kind}/{slug}/{filename}.md")
        }
    }
}

/// Build the absolute on-disk path for a chunk given the content root.
pub fn chunk_abs_path(
    content_root: &Path,
    source_kind: &str,
    source_id: &str,
    chunk_id: &str,
) -> PathBuf {
    let rel = chunk_rel_path(source_kind, source_id, chunk_id);
    let mut abs = content_root.to_path_buf();
    for component in rel.split('/') {
        abs.push(component);
    }
    abs
}

/// Convert a raw `source_id` into a filesystem-safe slug using only
/// `[a-z0-9_-]` characters.
///
/// Rules: lowercase; non-`[a-z0-9_-]` → `-`; collapse consecutive `-`; trim
/// leading/trailing separators; preserve interior `_`; truncate to 120 chars.
pub fn slugify_source_id(source_id: &str) -> String {
    let lower = source_id.to_lowercase();
    let mut out = String::with_capacity(lower.len().min(120));
    let mut last_dash = true;
    let mut pending_underscore = false;

    for ch in lower.chars() {
        if ch == '_' {
            if !last_dash {
                pending_underscore = true;
            }
        } else if ch.is_ascii_alphanumeric() {
            if pending_underscore {
                out.push('_');
                pending_underscore = false;
            }
            out.push(ch);
            last_dash = false;
        } else {
            pending_underscore = false;
            if !last_dash {
                out.push('-');
                last_dash = true;
            }
        }
    }
    let trimmed = out.trim_end_matches('-');
    let trimmed = trimmed.trim_end_matches('_');
    let truncated = truncate_at_char(trimmed, 120);
    if truncated.is_empty() {
        "unknown".to_string()
    } else {
        truncated.to_string()
    }
}

/// Truncate `s` to at most `max_chars` Unicode code points.
fn truncate_at_char(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

#[cfg(test)]
#[path = "paths_tests.rs"]
mod tests;
