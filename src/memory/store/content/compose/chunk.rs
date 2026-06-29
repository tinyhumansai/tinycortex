//! Chunk `.md` file composition and tag rewriting.

use crate::memory::chunks::{Chunk, SourceKind};
use crate::memory::store::content::compose::yaml::{
    split_front_matter, with_source_tag, yaml_scalar,
};

/// Compose the full file content (front-matter + body) for `chunk`.
///
/// Returns `(full_file_bytes, body_bytes)`. The caller writes `full_file_bytes`
/// to disk; `body_bytes` is what the SHA-256 is computed over.
pub fn compose_chunk_file(chunk: &Chunk) -> (Vec<u8>, Vec<u8>) {
    let front_matter = build_front_matter(chunk);
    let body = chunk.content.as_bytes().to_vec();

    let mut full = Vec::with_capacity(front_matter.len() + body.len());
    full.extend_from_slice(&front_matter);
    full.extend_from_slice(&body);

    (full, body)
}

/// Build the YAML front-matter block (including delimiters) as UTF-8 bytes.
fn build_front_matter(chunk: &Chunk) -> Vec<u8> {
    let meta = &chunk.metadata;
    let ts = meta.timestamp.to_rfc3339();
    let ts_start = meta.time_range.0.to_rfc3339();
    let ts_end = meta.time_range.1.to_rfc3339();

    let mut fm = String::new();
    fm.push_str("---\n");
    fm.push_str(&format!("source_kind: {}\n", meta.source_kind.as_str()));
    fm.push_str(&format!("source_id: {}\n", yaml_scalar(&meta.source_id)));
    if let Some(path_scope) = meta.path_scope.as_deref() {
        fm.push_str(&format!("path_scope: {}\n", yaml_scalar(path_scope)));
    }
    fm.push_str(&format!("seq: {}\n", chunk.seq_in_source));
    fm.push_str(&format!("owner: {}\n", yaml_scalar(&meta.owner)));
    fm.push_str(&format!("timestamp: {ts}\n"));
    fm.push_str(&format!("time_range_start: {ts_start}\n"));
    fm.push_str(&format!("time_range_end: {ts_end}\n"));

    if let Some(ref sr) = meta.source_ref {
        fm.push_str(&format!("source_ref: {}\n", yaml_scalar(&sr.value)));
    }

    // Always seed the source tag so the Obsidian graph filter can pick up
    // `source/<slug>` for every chunk regardless of the ingest-side tag list.
    let source_scope = meta.path_scope.as_deref().unwrap_or(&meta.source_id);
    let seeded_tags = with_source_tag(source_scope, &meta.tags);
    fm.push_str("tags:\n");
    for tag in &seeded_tags {
        fm.push_str(&format!("  - {}\n", yaml_scalar(tag)));
    }

    // Email-specific fields: participants list + Obsidian alias, parsed from
    // `gmail:{participants}` source_ids. Omitted when the format doesn't match.
    if meta.source_kind == SourceKind::Email {
        if let Some(addrs) = parse_gmail_participants_source_id(&meta.source_id) {
            fm.push_str("participants:\n");
            for addr in &addrs {
                fm.push_str(&format!("  - {}\n", yaml_scalar(addr)));
            }
            let alias = build_participants_alias(&addrs, chunk.seq_in_source);
            fm.push_str("aliases:\n");
            fm.push_str(&format!("  - {}\n", yaml_scalar(&alias)));
        }
    }

    fm.push_str("---\n");
    fm.into_bytes()
}

/// Parse a `gmail:{participants}` source_id into the list of participant
/// addresses. `participants` is `addr1|addr2|...` (sorted, deduped). Returns
/// `None` for legacy or malformed source_ids.
fn parse_gmail_participants_source_id(source_id: &str) -> Option<Vec<String>> {
    let (prefix, participants) = source_id.split_once(':')?;
    if prefix != "gmail" || participants.is_empty() {
        return None;
    }
    let addrs: Vec<String> = participants
        .split('|')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if addrs.is_empty() {
        None
    } else {
        Some(addrs)
    }
}

/// Build a human-readable alias for an email chunk's `aliases:` field.
fn build_participants_alias(addrs: &[String], seq: u32) -> String {
    let label = match addrs {
        [] => "unknown".to_string(),
        [only] => only.clone(),
        [first, second] => format!("{} <-> {}", first, second),
        [first, rest @ ..] => format!("{} <-> {} others", first, rest.len()),
    };
    format!("{}: chunk {}", label, seq)
}

/// Rewrite the `tags:` block in an existing file's front-matter, replacing it
/// with the new tag list while leaving the body unchanged.
pub fn rewrite_tags(file_bytes: &[u8], new_tags: &[String]) -> Result<Vec<u8>, String> {
    let content =
        std::str::from_utf8(file_bytes).map_err(|e| format!("file is not valid UTF-8: {e}"))?;

    let (front_matter, body) = split_front_matter(content)
        .ok_or_else(|| "cannot find front-matter delimiters".to_string())?;

    let new_fm = replace_tags_in_front_matter(front_matter, new_tags)?;

    let mut out = Vec::with_capacity(new_fm.len() + body.len() + 4);
    out.extend_from_slice(new_fm.as_bytes());
    out.extend_from_slice(body.as_bytes());
    Ok(out)
}

/// Replace the `tags:` stanza in a front-matter string, preserving delimiters.
fn replace_tags_in_front_matter(fm: &str, new_tags: &[String]) -> Result<String, String> {
    let replacement = if new_tags.is_empty() {
        "tags: []".to_string()
    } else {
        let mut s = "tags:".to_string();
        for tag in new_tags {
            s.push('\n');
            s.push_str(&format!("  - {}", yaml_scalar(tag)));
        }
        s
    };

    let lines: Vec<&str> = fm.lines().collect();
    let mut out_lines: Vec<&str> = Vec::new();
    let mut i = 0;
    let mut found = false;

    while i < lines.len() {
        let line = lines[i];
        if line == "tags: []" || line == "tags:" {
            found = true;
            i += 1;
            if line == "tags:" {
                while i < lines.len() && lines[i].starts_with("  - ") {
                    i += 1;
                }
            }
            continue;
        }
        out_lines.push(line);
        i += 1;
    }

    if !found {
        return Err("tags: key not found in front-matter".to_string());
    }

    let closing = out_lines
        .iter()
        .rposition(|l| *l == "---")
        .unwrap_or(out_lines.len());

    let mut result_lines: Vec<String> =
        out_lines[..closing].iter().map(|l| l.to_string()).collect();
    result_lines.push(replacement);
    result_lines.push("---".to_string());

    let mut result = result_lines.join("\n");
    result.push('\n');
    Ok(result)
}
