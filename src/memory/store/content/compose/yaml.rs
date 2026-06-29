//! YAML scalar helpers and front-matter parsing utilities.

/// Build the canonical Obsidian `source/<slug>` tag for a given source scope.
/// Used to seed the `tags:` block on every chunk and source-tree summary so the
/// Obsidian graph view can filter by source.
///
/// Slug rules match `slugify_source_id` so the tag matches the on-disk
/// `raw/<slug>/...` directory name byte-for-byte.
pub fn source_tag(scope: &str) -> String {
    use crate::memory::store::content::paths::slugify_source_id;
    format!("source/{}", slugify_source_id(scope))
}

/// Prepend the source tag to `tags`, dedup, and return the new list. Order is
/// preserved otherwise — `source/...` always comes first.
pub fn with_source_tag(scope: &str, tags: &[String]) -> Vec<String> {
    let st = source_tag(scope);
    let mut out = Vec::with_capacity(tags.len() + 1);
    out.push(st.clone());
    for t in tags {
        if t != &st {
            out.push(t.clone());
        }
    }
    out
}

/// Parse the value of a top-level YAML scalar field (e.g. `source_id`,
/// `tree_scope`, `tree_kind`) from a frontmatter string. Strips surrounding
/// double-quotes if present. Returns `None` if the key is not present at the
/// top level.
pub fn scan_fm_field(fm: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}: ");
    for raw in fm.lines() {
        // Skip indented lines (those are list items / nested mappings).
        if raw.starts_with(' ') || raw.starts_with('\t') {
            continue;
        }
        if let Some(rest) = raw.strip_prefix(&prefix) {
            let trimmed = rest.trim();
            if let Some(inner) = trimmed.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
                return Some(inner.replace("\\\"", "\"").replace("\\\\", "\\"));
            }
            return Some(trimmed.to_string());
        }
    }
    None
}

/// Split a file into `(front_matter, body)` at the second `---` delimiter.
///
/// Returns `None` if the file does not have the expected `---\n...\n---\n` form.
pub fn split_front_matter(content: &str) -> Option<(&str, &str)> {
    if !content.starts_with("---\n") {
        return None;
    }
    let rest = &content[4..]; // skip the opening `---\n`
    let close_idx = rest
        .find("\n---\n")
        .or_else(|| rest.strip_suffix("\n---").map(|r| r.len()))?;
    let fm_end = 4 + close_idx + 5; // include `\n---\n`
    debug_assert!(content.is_char_boundary(fm_end));
    Some((&content[..fm_end], &content[fm_end..]))
}

/// Format a string as an unquoted YAML scalar when safe, or as a
/// double-quoted string when it contains special characters.
pub fn yaml_scalar(s: &str) -> String {
    let needs_quoting = s.is_empty()
        || s.trim() != s
        || s.starts_with(|c: char| {
            matches!(
                c,
                '&' | '*' | '?' | '|' | '-' | '<' | '>' | '=' | '!' | '%' | '@' | '`'
            )
        })
        || s.contains([':', '#', '[', ']', '{', '}', '"', '\'']);

    if needs_quoting {
        let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
        format!("\"{escaped}\"")
    } else {
        s.to_string()
    }
}
