//! Serialization, identity, and patch helpers for the git ledger.

use std::collections::HashMap;

use anyhow::{Context, Result};
use git2::{Oid, Signature, Time};

use super::ledger::{SnapshotMeta, MAX_TEXT_DIFF_CHARS, READ_MARKER_PREFIX, SIG_EMAIL, SIG_NAME};
use super::types::Checkpoint;

// ── Free helpers ───────────────────────────────────────────────────────

pub(super) fn signature(at_ms: i64) -> Result<Signature<'static>> {
    let time = Time::new(at_ms / 1000, 0);
    Signature::new(SIG_NAME, SIG_EMAIL, &time).context("build git signature")
}

pub(super) fn read_marker_ref(source_id: &str) -> String {
    format!("{READ_MARKER_PREFIX}{}", encode_source_id(source_id))
}

pub(super) fn build_commit_message(
    meta: &SnapshotMeta,
    item_count: u32,
    taken_at_ms: i64,
) -> String {
    format!(
        "snapshot: {source} ({count} item(s))\n\n\
         Source-Id: {source}\n\
         Source-Kind: {kind}\n\
         Source-Label: {label}\n\
         Trigger: {trigger}\n\
         Item-Count: {count}\n\
         Taken-At-Ms: {taken}\n",
        source = meta.source_id,
        kind = meta.source_kind,
        label = sanitize_trailer(&meta.label),
        trigger = meta.trigger.as_str(),
        count = item_count,
        taken = taken_at_ms,
    )
}

/// Trailer values are single-line; collapse newlines so a multi-line label
/// can't corrupt the trailer block.
pub(super) fn sanitize_trailer(s: &str) -> String {
    s.replace(['\n', '\r'], " ")
}

/// Parse `Key: value` trailer lines from a commit message into a
/// lowercase-keyed map.
pub(super) fn parse_trailers(message: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let trailer_block = message
        .trim_end()
        .rsplit_once("\n\n")
        .map_or(message, |(_, block)| block);
    for line in trailer_block.lines() {
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim().to_ascii_lowercase();
            if !key.is_empty() && !key.contains(' ') {
                map.insert(key, v.trim().to_string());
            }
        }
    }
    map
}

pub(super) fn validate_source_id(source_id: &str) -> Result<()> {
    anyhow::ensure!(!source_id.trim().is_empty(), "source id must not be blank");
    anyhow::ensure!(
        !source_id.chars().any(char::is_control),
        "source id must not contain control characters"
    );
    Ok(())
}

pub(super) fn checkpoint_message(
    label: &str,
    snapshot_ids: &[String],
    created_at_ms: i64,
) -> String {
    let payload = serde_json::json!({
        "label": label,
        "snapshot_ids": snapshot_ids,
        "created_at_ms": created_at_ms,
    });
    payload.to_string()
}

pub(super) fn checkpoint_from_message(id: &str, message: &str) -> Result<Checkpoint> {
    let value: serde_json::Value = serde_json::from_str(message.trim())
        .with_context(|| format!("checkpoint '{id}' has invalid JSON metadata"))?;
    let label = value
        .get("label")
        .and_then(|v| v.as_str())
        .with_context(|| format!("checkpoint '{id}' is missing string label"))?;
    let created_at_ms = value
        .get("created_at_ms")
        .and_then(|v| v.as_i64())
        .with_context(|| format!("checkpoint '{id}' is missing integer created_at_ms"))?;
    let snapshot_values = value
        .get("snapshot_ids")
        .and_then(|v| v.as_array())
        .with_context(|| format!("checkpoint '{id}' is missing snapshot_ids array"))?;
    let snapshot_ids = snapshot_values
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_string)
                .with_context(|| format!("checkpoint '{id}' contains a non-string snapshot id"))
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(Checkpoint {
        id: id.to_string(),
        label: label.to_string(),
        created_at_ms,
        snapshot_ids,
    })
}

/// A git blob oid as a content hash, or `None` for the zero oid (absent side).
pub(super) fn oid_hash(oid: Oid) -> Option<String> {
    if oid.is_zero() {
        None
    } else {
        Some(oid.to_string())
    }
}

/// Render a single delta's unified patch, truncated to [`MAX_TEXT_DIFF_CHARS`].
pub(super) fn patch_text(diff: &git2::Diff, delta_idx: usize) -> Option<String> {
    let mut patch = git2::Patch::from_diff(diff, delta_idx).ok().flatten()?;
    let buf = patch.to_buf().ok()?;
    // git2 0.21: Buf::as_str() returns Result<&str, Utf8Error>.
    let text = buf.as_str().ok()?;
    if text.trim().is_empty() {
        None
    } else {
        Some(truncate(text, MAX_TEXT_DIFF_CHARS))
    }
}

/// Encode an item id into a single git-safe path component. Bytes outside
/// `[A-Za-z0-9._-]` become `%XX`; an `i_` prefix keeps the result clear of the
/// reserved names `.`/`..`/empty. Reversible via [`decode_item_id`].
pub(crate) fn encode_item_id(item_id: &str) -> String {
    let mut out = String::with_capacity(item_id.len() + 2);
    out.push_str("i_");
    for &b in item_id.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-') {
            out.push(b as char);
        } else {
            out.push('%');
            out.push_str(&format!("{b:02X}"));
        }
    }
    out
}

/// Encode a source id for use as a top-level git tree entry and read-marker
/// ref component. Same reversible encoding as item ids; kept as a named helper
/// so call sites make the source-vs-item boundary explicit.
pub(crate) fn encode_source_id(source_id: &str) -> String {
    encode_item_id(source_id)
}

/// Inverse of [`encode_item_id`].
pub(crate) fn decode_item_id(encoded: &str) -> String {
    let body = encoded.strip_prefix("i_").unwrap_or(encoded);
    let bytes = body.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub(super) fn truncate(s: &str, max_chars: usize) -> String {
    if s.len() <= max_chars {
        s.to_string()
    } else {
        let mut end = max_chars;
        while !s.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        format!("{}…(truncated)", &s[..end])
    }
}

/// Derive a human-readable title from item content: the first non-empty line
/// (Markdown heading markers stripped), bounded. Falls back to the item id.
pub(super) fn derive_title(item_id: &str, content: &str) -> String {
    let first_line = content
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(|l| l.trim_start_matches('#').trim());
    match first_line {
        Some(l) if !l.is_empty() => truncate(l, 120),
        _ => item_id.to_string(),
    }
}
