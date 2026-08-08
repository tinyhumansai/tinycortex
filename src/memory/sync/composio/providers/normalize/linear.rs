//! Linear host normalization helpers — result extraction, issue-title extraction,
//! viewer identity, cursor extraction, and time utilities.
//!
//! Linear's GraphQL API (and therefore Composio's wrapping of it) returns
//! connection-style lists (`{ nodes: [...], pageInfo: {...} }`) at the top
//! level or nested under `data`. The functions here walk the union of
//! common shapes so the provider does not have to branch per Composio
//! envelope variant.

use serde_json::Value;

use super::helpers::pick_str;

/// Walk the Composio response envelope for Linear issue list results.
///
/// Linear's list endpoints return `{ nodes: [...] }` or
/// `{ issues: { nodes: [...] } }` shapes; Composio may re-wrap the
/// upstream payload under `data` or `data.data`. We probe each shape
/// in order and return the first array we find.
pub fn extract_issues(data: &Value) -> Vec<Value> {
    let candidates = [
        data.pointer("/data/nodes"),
        data.pointer("/nodes"),
        data.pointer("/data/issues/nodes"),
        data.pointer("/issues/nodes"),
        data.pointer("/data/data/nodes"),
        data.pointer("/data/data/issues/nodes"),
        data.pointer("/data/results"),
        data.pointer("/results"),
        data.pointer("/data/items"),
        data.pointer("/items"),
    ];
    for cand in candidates.into_iter().flatten() {
        if let Some(arr) = cand.as_array() {
            return arr.clone();
        }
    }
    Vec::new()
}

/// Extract a human-readable title from a Linear issue object.
///
/// Linear issues store the name at `title` (or `data.title` after
/// Composio envelope wrapping). Falls back to `name` / `identifier`
/// so the chunk remains identifiable even for unusual response shapes.
pub fn extract_issue_title(issue: &Value) -> Option<String> {
    pick_str(
        issue,
        &[
            "title",
            "data.title",
            "name",
            "data.name",
            "identifier",
            "data.identifier",
        ],
    )
}

/// Extract a stable cursor timestamp from a Linear issue object.
///
/// Linear uses ISO-8601 strings for timestamps (`updatedAt`). We keep
/// the value as a string so lexicographic comparison against the stored
/// cursor is valid.
pub fn extract_issue_updated(issue: &Value) -> Option<String> {
    pick_str(
        issue,
        &[
            "updatedAt",
            "data.updatedAt",
            "updated_at",
            "data.updated_at",
        ],
    )
}

/// Extract the viewer (authenticated user) object from a
/// `LINEAR_LIST_LINEAR_USERS { isMe: true }` response.
///
/// Linear's GraphQL viewer endpoint returns `{ nodes: [{ id, email, … }] }`.
/// Composio may wrap this under `data` or `data.data`. We probe each
/// shape and return the first element of the nodes array, falling back
/// to the payload itself if it looks like a direct user object (has
/// `id` or `email`).
pub fn extract_viewer(data: &Value) -> Option<Value> {
    let array_candidates = [
        data.pointer("/data/nodes"),
        data.pointer("/nodes"),
        data.pointer("/data/data/nodes"),
        data.pointer("/data/users/nodes"),
    ];
    for cand in array_candidates.into_iter().flatten() {
        if let Some(arr) = cand.as_array() {
            if let Some(first) = arr.first() {
                return Some(first.clone());
            }
        }
    }
    // Fallback: if the payload itself looks like a user object, return it.
    if data.get("id").is_some() || data.get("email").is_some() {
        return Some(data.clone());
    }
    None
}

/// Extract the viewer's ID string from a `LINEAR_LIST_LINEAR_USERS`
/// response. Returns `None` if the payload does not contain a
/// recognizable user ID.
pub fn extract_viewer_id(data: &Value) -> Option<String> {
    let viewer = extract_viewer(data)?;
    pick_str(&viewer, &["id", "data.id"])
}

/// Extract a pagination cursor from a Linear connection `pageInfo` block.
///
/// Returns `Some(endCursor)` only when `hasNextPage` is `true`;
/// `None` when the last page has been reached or when the envelope does
/// not carry `pageInfo` at all.
pub fn extract_pagination_cursor(data: &Value) -> Option<String> {
    let page_info_candidates = [
        data.pointer("/data/pageInfo"),
        data.pointer("/pageInfo"),
        data.pointer("/data/data/pageInfo"),
        data.pointer("/data/issues/pageInfo"),
    ];
    for cand in page_info_candidates.into_iter().flatten() {
        let has_next = cand
            .get("hasNextPage")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if has_next {
            if let Some(cursor) = cand.get("endCursor").and_then(|v| v.as_str()) {
                let trimmed = cursor.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
    }
    None
}

/// Current wall-clock time in milliseconds since the UNIX epoch.
pub fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
#[path = "linear_tests.rs"]
mod tests;
