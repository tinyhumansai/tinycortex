//! Slack response cursor, mention, and timestamp parsing.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde_json::Value;

use super::common::first_array;

/// Return the cached matcher for Slack `<@USERID>` mentions.
pub(super) fn mention_regex() -> &'static regex::Regex {
    static REGEX: OnceLock<regex::Regex> = OnceLock::new();
    REGEX.get_or_init(|| regex::Regex::new(r"<@(U[A-Z0-9]+)>").expect("Slack mention regex"))
}

/// Replace Slack mention tokens with resolved display names, falling back to
/// the raw user id when the optional user map has no match.
pub(super) fn replace_mentions(
    text: &str,
    users: Option<&serde_json::Map<String, Value>>,
) -> String {
    mention_regex()
        .replace_all(text, |captures: &regex::Captures<'_>| {
            let id = &captures[1];
            let resolved = users
                .and_then(|users| users.get(id))
                .and_then(Value::as_str)
                .unwrap_or(id);
            format!("@{resolved}")
        })
        .into_owned()
}

/// Read the first non-blank next cursor across supported response envelopes.
pub(super) fn next_cursor(data: &Value) -> Option<String> {
    [
        "/data/response_metadata/next_cursor",
        "/response_metadata/next_cursor",
        "/data/next_cursor",
        "/next_cursor",
        "/data/data/response_metadata/next_cursor",
    ]
    .iter()
    .find_map(|path| data.pointer(path).and_then(Value::as_str))
    .map(str::trim)
    .filter(|cursor| !cursor.is_empty())
    .map(str::to_owned)
}

/// Extract Slack search matches across legacy and nested response envelopes.
pub(super) fn search_matches(data: &Value) -> Vec<Value> {
    first_array(
        data,
        &[
            "/data/messages/matches",
            "/messages/matches",
            "/data/data/messages/matches",
            "/messages",
        ],
    )
}

/// Extract the search page count, defaulting to one when paging is absent.
pub(super) fn search_total_pages(data: &Value) -> u32 {
    [
        "/data/messages/paging/pages",
        "/messages/paging/pages",
        "/data/data/messages/paging/pages",
        "/pages",
    ]
    .iter()
    .find_map(|path| data.pointer(path).and_then(Value::as_u64))
    .unwrap_or(1) as u32
}

/// Decode persisted per-scope cursors, returning an empty map for absent or
/// malformed JSON so synchronization can restart safely.
pub(super) fn decode_cursors(raw: Option<&str>) -> BTreeMap<String, String> {
    raw.and_then(|raw| serde_json::from_str(raw).ok())
        .unwrap_or_default()
}

/// Parse Slack's `seconds.fraction` timestamp into numeric components.
/// Missing fractions become zero; malformed numeric components return `None`.
pub(super) fn parse_ts(ts: &str) -> Option<(i64, u64)> {
    let mut parts = ts.splitn(2, '.');
    Some((
        parts.next()?.parse().ok()?,
        parts.next().unwrap_or("0").parse().ok()?,
    ))
}
