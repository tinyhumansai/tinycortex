//! Post-extraction tag rewriting for chunk `.md` files.
//!
//! After extraction produces entities, each is converted to an Obsidian-style
//! hierarchical tag (`kind/Value`) and written into the `tags:` block in the
//! file's front-matter. The body bytes (and therefore the SHA-256) are never
//! changed — only the front-matter is rewritten.
//!
//! The Config/SQLite-aware `update_summary_tags` (which reads entity rows from
//! the entity index and resolves the on-disk path through the chunk store) is
//! **deferred** along with the rest of the SQLite chunk-store integration.

use std::path::Path;

use super::compose::{rewrite_tags, scan_fm_field, source_tag, split_front_matter};

/// Rewrite the `tags:` block in a chunk's on-disk `.md` file.
///
/// `abs_path` — absolute path to the chunk file. `tags` — new Obsidian
/// `kind/Value` tag strings. Atomic: written to a sibling temp path and renamed
/// over the original. A missing file is a no-op (`Ok(())`).
///
/// Unlike the initial chunk write, tag rewrites MAY overwrite an existing file:
/// the immutability contract covers the **body** only.
pub fn update_chunk_tags(abs_path: &Path, tags: &[String]) -> anyhow::Result<()> {
    if !abs_path.exists() {
        return Ok(());
    }

    let old_bytes =
        std::fs::read(abs_path).map_err(|e| anyhow::anyhow!("read {:?}: {e}", abs_path))?;

    // Re-seed the `source/<slug>` tag so it survives every rewrite, pulled from
    // the existing frontmatter's `path_scope:` / `source_id:` field.
    let augmented = augment_with_source_tag_for_chunk(&old_bytes, tags);
    let new_bytes = rewrite_tags(&old_bytes, &augmented)
        .map_err(|e| anyhow::anyhow!("rewrite_tags {:?}: {e}", abs_path))?;

    let parent = abs_path.parent().unwrap_or_else(|| Path::new("."));
    let tmp_name = format!(".tmp_tags_{}.md", crate_temp_id());
    let tmp_path = parent.join(&tmp_name);

    {
        use std::io::Write;
        let mut f = std::fs::File::create(&tmp_path)
            .map_err(|e| anyhow::anyhow!("create tag-rewrite tempfile {:?}: {e}", tmp_path))?;
        f.write_all(&new_bytes)
            .map_err(|e| anyhow::anyhow!("write tag-rewrite tempfile {:?}: {e}", tmp_path))?;
        f.sync_all()
            .map_err(|e| anyhow::anyhow!("fsync tag-rewrite tempfile {:?}: {e}", tmp_path))?;
    }

    std::fs::rename(&tmp_path, abs_path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp_path);
        anyhow::anyhow!("rename tag-rewrite {:?} -> {:?}: {e}", tmp_path, abs_path)
    })?;

    Ok(())
}

/// Slugify an entity kind string for an Obsidian hierarchical tag.
///
/// Output: lowercase, non-alphanumeric → `-`, collapsed, trimmed.
pub fn slugify_tag_kind(kind: &str) -> String {
    slugify_tag_component(kind)
}

/// Slugify an entity value, capitalising the first letter of each word so
/// values are visually distinct from kinds. `"alice johnson"` → `"Alice-Johnson"`.
pub fn slugify_tag_value(value: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut current = String::new();

    for ch in value.chars() {
        if ch.is_alphanumeric() || ch == '_' {
            current.push(ch);
        } else if !current.is_empty() {
            parts.push(capitalise(&current));
            current.clear();
        }
    }
    if !current.is_empty() {
        parts.push(capitalise(&current));
    }

    let joined = parts.join("-");
    if joined.is_empty() {
        "unknown".to_string()
    } else {
        joined
    }
}

/// Build an Obsidian-style `kind/Value` tag from raw entity kind + surface.
pub fn entity_tag(kind: &str, surface: &str) -> String {
    format!("{}/{}", slugify_tag_kind(kind), slugify_tag_value(surface))
}

/// Shared slugify core for [`slugify_tag_kind`]: lowercase, non-`[a-z0-9_]` runs
/// collapse to a single `-`, trailing `-` trimmed, empty result → `"unknown"`.
fn slugify_tag_component(s: &str) -> String {
    let lower = s.to_lowercase();
    let mut out = String::new();
    let mut last_dash = true;
    for ch in lower.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out.push(ch);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed = out.trim_end_matches('-');
    if trimmed.is_empty() {
        "unknown".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Uppercase the first `char` of `s` and leave the rest untouched.
fn capitalise(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => {
            let upper: String = first.to_uppercase().collect();
            upper + chars.as_str()
        }
    }
}

/// Read `path_scope:` / `source_id:` out of a chunk file's frontmatter and
/// return `[source/<slug>, ...tags]` (deduped). Falls back to `tags` unchanged
/// if the frontmatter can't be parsed.
fn augment_with_source_tag_for_chunk(file_bytes: &[u8], tags: &[String]) -> Vec<String> {
    let Ok(text) = std::str::from_utf8(file_bytes) else {
        return tags.to_vec();
    };
    let Some((fm, _body)) = split_front_matter(text) else {
        return tags.to_vec();
    };
    let Some(source_scope) =
        scan_fm_field(fm, "path_scope").or_else(|| scan_fm_field(fm, "source_id"))
    else {
        return tags.to_vec();
    };
    let st = source_tag(&source_scope);
    let mut out = Vec::with_capacity(tags.len() + 1);
    out.push(st.clone());
    for t in tags {
        if t != &st {
            out.push(t.clone());
        }
    }
    out
}

/// Generate a collision-resistant temp-file suffix.
fn crate_temp_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

#[cfg(test)]
#[path = "tags_tests.rs"]
mod tests;
