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
                return Some(unescape_double_quoted(inner));
            }
            return Some(trimmed.to_string());
        }
    }
    None
}

/// Decode a double-quoted YAML scalar body (the text between the surrounding
/// `"`) produced by [`yaml_scalar`] back into its original value.
///
/// This is a single left-to-right pass so nested escapes round-trip correctly
/// regardless of ordering — a sequence of `str::replace` calls would corrupt
/// values such as `\\"` where one escape's output looks like another's input.
/// Recognised escapes mirror [`yaml_scalar`]: `\\`, `\"`, `\n`, `\r`, `\t`.
/// Any other `\x` sequence is preserved verbatim (backslash + char).
fn unescape_double_quoted(inner: &str) -> String {
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('\\') => out.push('\\'),
            Some('"') => out.push('"'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Split a file into `(front_matter, body)` at the closing `---` delimiter.
///
/// Accepts both canonical forms:
/// - `---\n...\n---\n<body>` — closing fence followed by a body, and
/// - `---\n...\n---` — closing fence at EOF with an empty body (no trailing
///   newline).
///
/// Returns `None` (never panics) if the file does not open with `---\n` or has
/// no closing fence.
pub fn split_front_matter(content: &str) -> Option<(&str, &str)> {
    if !content.starts_with("---\n") {
        return None;
    }
    let rest = &content[4..]; // skip the opening `---\n`

    // The closing fence is either `\n---\n` (a body follows) or `\n---` at EOF
    // (empty body). These have different lengths, so the byte arithmetic must
    // branch: `\n---\n` is 5 bytes, `\n---` is 4. Using a fixed `+5` for the
    // EOF case overruns the buffer and panics.
    let fm_end = if let Some(idx) = rest.find("\n---\n") {
        4 + idx + 5
    } else {
        let prefix = rest.strip_suffix("\n---")?;
        4 + prefix.len() + 4
    };
    debug_assert!(content.is_char_boundary(fm_end));
    Some((&content[..fm_end], &content[fm_end..]))
}

/// Format a string as an unquoted YAML scalar when safe, or as a
/// double-quoted string when it contains special characters.
///
/// Any control character (newline, carriage return, tab, etc.) forces quoting
/// and, for line-breaking characters, escaping. This is a security boundary:
/// provider-controlled values (`source_id`, `owner`, `source_ref`, tags) must
/// not be able to inject additional front-matter lines or terminate the block
/// early with an embedded `\n---\n`. Escapes are decoded by
/// the matching scanner unescape path, so values round-trip.
pub fn yaml_scalar(s: &str) -> String {
    let needs_quoting = s.is_empty()
        || s.trim() != s
        || s.chars().any(char::is_control)
        || s.starts_with(|c: char| {
            matches!(
                c,
                '&' | '*' | '?' | '|' | '-' | '<' | '>' | '=' | '!' | '%' | '@' | '`'
            )
        })
        || s.contains([':', '#', '[', ']', '{', '}', '"', '\'']);

    if needs_quoting {
        let mut escaped = String::with_capacity(s.len() + 2);
        for c in s.chars() {
            match c {
                '\\' => escaped.push_str("\\\\"),
                '"' => escaped.push_str("\\\""),
                '\n' => escaped.push_str("\\n"),
                '\r' => escaped.push_str("\\r"),
                '\t' => escaped.push_str("\\t"),
                _ => escaped.push(c),
            }
        }
        format!("\"{escaped}\"")
    } else {
        s.to_string()
    }
}
