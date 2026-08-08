//! Slack-specific post-processing of Composio action responses.
//!
//! Composio's Slack responses are verbose API envelopes. This module
//! rewrites each supported action's response into a slim, stable shape
//! that the ingest pipeline and enrichers can consume without walking
//! Composio's unstable nested envelopes.
//!
//! ## Supported slugs
//!
//! - `SLACK_FETCH_CONVERSATION_HISTORY` — reshapes into top-level
//!   `messages[]` with `{ ts, user, text, thread_ts, channel_id }`.
//!   Empty-text messages are dropped. `channel_id` is absent here (it's
//!   in the request, not the response); the caller injects it via the
//!   enricher in
//!   [`crate::memory::sync::composio::providers::SlackSyncPipeline`].
//!
//! - `SLACK_LIST_CONVERSATIONS` — reshapes into top-level `channels[]`
//!   with `{ id, name, is_private }` per channel. Entries with an empty
//!   id are dropped.
//!
//! - `SLACK_SEARCH_MESSAGES` — reshapes `messages.matches[]` (possibly
//!   nested) into top-level `messages[]` with `{ ts, user, text,
//!   thread_ts, channel_id }`. `channel_id` is pulled from each match's
//!   `channel.id` field. `paging.pages` is preserved at top-level for
//!   caller pagination.
//!
//! ## Design note: user-id resolution is NOT here
//!
//! `SlackUsers` is a per-sync cache built from a separate API call —
//! not a function of any individual response. Resolving user ids
//! happens in
//! [`crate::memory::sync::composio::providers::SlackSyncPipeline`]
//! (the enricher layer), keeping this module purely data-shape–oriented.
//! This matches Gmail's pattern of "post_process is data-only".
//!
//! Unknown slugs are silently no-ops so new Composio actions don't
//! break the provider.

use serde_json::{Map, Value};

/// Entry point called from `SlackProvider::post_process_action_result`.
///
/// Dispatches on the Composio action slug and rewrites `data` in place.
/// Unknown slugs are silently ignored.
pub fn post_process(slug: &str, _arguments: Option<&Value>, data: &mut Value) {
    log::debug!("[composio:slack][post-process] slug={slug}");
    match slug {
        "SLACK_FETCH_CONVERSATION_HISTORY" => reshape_fetch_history(data),
        "SLACK_LIST_CONVERSATIONS" => reshape_list_conversations(data),
        "SLACK_SEARCH_MESSAGES" => reshape_search_messages(data),
        _ => {
            log::debug!("[composio:slack][post-process] unknown slug={slug}, passing through");
        }
    }
}

// ─── SLACK_FETCH_CONVERSATION_HISTORY ──────────────────────────────────────

/// Rewrite a `SLACK_FETCH_CONVERSATION_HISTORY` response in place.
///
/// Walks possible nested envelopes (`/data/messages`, `/messages`,
/// `/data/data/messages`) to find the raw messages array, drops messages
/// with empty `text`, and emits a slim `{ ts, user, text, thread_ts }`
/// shape under a top-level `messages[]` key. The consumed nested array is
/// removed from the payload so the raw verbose rows don't linger alongside
/// the slim copy. The caller injects `channel_id` via
/// [`super::sync::extract_messages`].
fn reshape_fetch_history(data: &mut Value) {
    let arr = take_array(
        data,
        &["/data/messages", "/messages", "/data/data/messages"],
        0,
    );
    let slim: Vec<Value> = arr.into_iter().filter_map(slim_history_message).collect();
    let obj = ensure_object(data);
    obj.insert("messages".to_string(), Value::Array(slim));
    log::debug!("[composio:slack][post-process] SLACK_FETCH_CONVERSATION_HISTORY reshaped");
}

fn slim_history_message(raw: Value) -> Option<Value> {
    let text = raw
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if text.is_empty() {
        return None;
    }
    let mut out = Map::new();
    if let Some(ts) = raw.get("ts") {
        out.insert("ts".into(), ts.clone());
    } else {
        return None; // ts is required — no ts means we can't cursor or archive
    }
    if let Some(user) = raw.get("user").or_else(|| raw.get("bot_id")) {
        out.insert("user".into(), user.clone());
    }
    out.insert("text".into(), Value::String(text.to_string()));
    if let Some(thread_ts) = raw.get("thread_ts") {
        out.insert("thread_ts".into(), thread_ts.clone());
    }
    if let Some(permalink) = raw.get("permalink") {
        out.insert("permalink".into(), permalink.clone());
    }
    Some(Value::Object(out))
}

/// Find the first array at any of `candidates`, remove that field (plus
/// `envelope_depth` ancestor object envelopes) from `data`, and return the
/// array. Removing the consumed nested payload keeps the reshaped output from
/// carrying duplicate raw rows.
fn take_array(data: &mut Value, candidates: &[&str], envelope_depth: usize) -> Vec<Value> {
    for path in candidates {
        let arr = match data.pointer(path).and_then(|v| v.as_array().cloned()) {
            Some(a) => a,
            None => continue,
        };
        let mut remove_path = path.to_string();
        for _ in 0..envelope_depth {
            remove_path = match remove_path.rsplit_once('/') {
                Some((parent, _)) => parent.to_string(),
                None => break,
            };
        }
        remove_nested(data, &remove_path);
        return arr;
    }
    Vec::new()
}

/// Remove the field at `path` from `data`, pruning any ancestor object that
/// the removal left empty so a consumed `data` envelope disappears entirely
/// instead of lingering as `{}`.
fn remove_nested(data: &mut Value, path: &str) {
    let segments: Vec<&str> = path
        .trim_start_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    if segments.is_empty() {
        return;
    }

    // Remove the leaf field.
    let mut current = &mut *data;
    for seg in &segments[..segments.len() - 1] {
        current = match current.get_mut(*seg) {
            Some(next) => next,
            None => return,
        };
    }
    if let Value::Object(map) = current {
        map.remove(segments[segments.len() - 1]);
    }

    // Prune empty object ancestors, deepest first.
    for depth in (0..segments.len().saturating_sub(1)).rev() {
        // Re-walk to the object at `segments[..=depth]`.
        let mut ancestor = &mut *data;
        for seg in &segments[..=depth] {
            ancestor = match ancestor.get_mut(*seg) {
                Some(next) => next,
                None => return,
            };
        }
        if !matches!(ancestor, Value::Object(m) if m.is_empty()) {
            break;
        }
        // Remove it from its parent (`segments[..depth]`). For `depth == 0`
        // the parent is the top-level object, so an emptied `data` envelope
        // key disappears entirely.
        let mut parent = &mut *data;
        for seg in &segments[..depth] {
            parent = match parent.get_mut(*seg) {
                Some(next) => next,
                None => return,
            };
        }
        if let Value::Object(map) = parent {
            map.remove(segments[depth]);
        }
    }
}

// ─── SLACK_LIST_CONVERSATIONS ───────────────────────────────────────────────

/// Rewrite a `SLACK_LIST_CONVERSATIONS` response in place.
///
/// Reshapes into a top-level `channels[]` with `{ id, name, is_private }`
/// per channel; entries with an empty id are dropped.
fn reshape_list_conversations(data: &mut Value) {
    let arr = take_array(
        data,
        &[
            "/data/channels",
            "/channels",
            "/data/data/channels",
            "/data/conversations",
            "/conversations",
        ],
        0,
    );

    let slim: Vec<Value> = arr.into_iter().filter_map(slim_channel).collect();
    let obj = ensure_object(data);
    obj.insert("channels".to_string(), Value::Array(slim));
    log::debug!("[composio:slack][post-process] SLACK_LIST_CONVERSATIONS reshaped");
}

fn slim_channel(raw: Value) -> Option<Value> {
    let id = raw.get("id").and_then(|v| v.as_str()).unwrap_or("").trim();
    if id.is_empty() {
        return None;
    }
    let name = raw
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or(id)
        .trim();
    let is_private = raw
        .get("is_private")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    Some(Value::Object({
        let mut m = Map::new();
        m.insert("id".into(), Value::String(id.to_string()));
        m.insert("name".into(), Value::String(name.to_string()));
        m.insert("is_private".into(), Value::Bool(is_private));
        m
    }))
}

// ─── SLACK_SEARCH_MESSAGES ──────────────────────────────────────────────────

/// Rewrite a `SLACK_SEARCH_MESSAGES` response in place.
///
/// Reshapes `messages.matches[]` (possibly nested under one or two
/// `data` envelopes) into top-level `messages[]`. `channel_id` is pulled
/// from each match's `channel.id` field. `paging.pages` is preserved at
/// top-level under `pages` for the caller to drive pagination.
fn reshape_search_messages(data: &mut Value) {
    // Preserve paging info before mutating data (take_array below removes the
    // envelope that carries it).
    let pages = [
        data.pointer("/data/messages/paging/pages"),
        data.pointer("/messages/paging/pages"),
        data.pointer("/data/data/messages/paging/pages"),
    ]
    .into_iter()
    .flatten()
    .find_map(|v| v.as_u64())
    .unwrap_or(1);

    // Envelope depth 1 removes the `messages` object (matches + paging) that
    // held the consumed rows, not just the `matches` array.
    let arr = take_array(
        data,
        &[
            "/data/messages/matches",
            "/messages/matches",
            "/data/data/messages/matches",
        ],
        1,
    );

    let slim: Vec<Value> = arr.into_iter().filter_map(slim_search_match).collect();
    let obj = ensure_object(data);
    obj.insert("messages".to_string(), Value::Array(slim));
    obj.insert("pages".to_string(), Value::Number(pages.into()));
    log::debug!("[composio:slack][post-process] SLACK_SEARCH_MESSAGES reshaped");
}

fn slim_search_match(raw: Value) -> Option<Value> {
    let text = raw
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if text.is_empty() {
        return None;
    }
    let ts = raw.get("ts")?;
    let channel_id = raw
        .pointer("/channel/id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();

    let mut out = Map::new();
    out.insert("ts".into(), ts.clone());
    if let Some(user) = raw.get("user").or_else(|| raw.get("bot_id")) {
        out.insert("user".into(), user.clone());
    }
    out.insert("text".into(), Value::String(text.to_string()));
    if let Some(thread_ts) = raw.get("thread_ts") {
        out.insert("thread_ts".into(), thread_ts.clone());
    }
    if !channel_id.is_empty() {
        out.insert("channel_id".into(), Value::String(channel_id.to_string()));
    }
    if let Some(permalink) = raw.get("permalink") {
        out.insert("permalink".into(), permalink.clone());
    }
    Some(Value::Object(out))
}

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Ensure `data` is a JSON object, replacing it with an empty object if
/// not. Returns a mutable ref to the inner map.
fn ensure_object(data: &mut Value) -> &mut Map<String, Value> {
    if !data.is_object() {
        *data = Value::Object(Map::new());
    }
    data.as_object_mut().unwrap()
}

#[cfg(test)]
#[path = "slack_post_process_tests.rs"]
mod tests;
