//! ClickUp host normalization helpers — result extraction, task-title extraction,
//! and time utilities.
//!
//! ClickUp's REST API (and therefore Composio's wrapping of it) returns
//! task lists in a small handful of shapes depending on which endpoint
//! is called. The functions here walk the union of common shapes so the
//! provider doesn't have to branch per Composio envelope variant.

use serde_json::Value;

use super::helpers::pick_str;

/// Walk the Composio response envelope for ClickUp task list results.
///
/// ClickUp's "filtered team tasks" endpoint returns `{ "tasks": [...] }`
/// at the top level; Composio re-wraps the upstream payload under
/// `data` or `data.data` depending on the action. We probe each shape
/// in order and return the first array we find.
pub fn extract_tasks(data: &Value) -> Vec<Value> {
    let candidates = [
        data.pointer("/data/tasks"),
        data.pointer("/tasks"),
        data.pointer("/data/data/tasks"),
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

/// Extract a human-readable title from a ClickUp task object.
///
/// ClickUp tasks store the name at `name` (or `data.name` after Composio
/// envelope wrapping). When the name is missing we fall back to the
/// task ID so chunks remain identifiable.
pub fn extract_task_name(task: &Value) -> Option<String> {
    pick_str(task, &["name", "data.name", "title", "data.title"])
}

/// Extract a stable cursor timestamp (milliseconds since epoch as a
/// string) from a ClickUp task object.
///
/// The ClickUp API returns `date_updated` as a stringified epoch ms
/// (e.g. `"1733412345678"`); we keep it as a string so lexicographic
/// comparison against the stored cursor remains valid as long as the
/// length doesn't change (it won't until year 33658).
pub fn extract_task_updated(task: &Value) -> Option<String> {
    pick_str(
        task,
        &[
            "date_updated",
            "data.date_updated",
            "updated_at",
            "data.updated_at",
            "dateUpdated",
            "data.dateUpdated",
        ],
    )
}

/// Current wall-clock time in milliseconds since the UNIX epoch.
pub fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Extract the authorized user's numeric ID from the
/// `CLICKUP_GET_AUTHORIZED_USER` response.
///
/// Composio wraps the upstream `{"user": {"id": …}}` shape; this walker
/// is defensive against both raw and wrapped payloads. Returns the ID
/// as a string because `CLICKUP_GET_FILTERED_TEAM_TASKS` accepts the
/// `assignees` filter as a string array.
pub fn extract_user_id(data: &Value) -> Option<String> {
    let candidates = [
        data.pointer("/user/id"),
        data.pointer("/data/user/id"),
        data.pointer("/id"),
        data.pointer("/data/id"),
    ];
    for cand in candidates.into_iter().flatten() {
        if let Some(n) = cand.as_u64() {
            return Some(n.to_string());
        }
        if let Some(n) = cand.as_i64() {
            return Some(n.to_string());
        }
        if let Some(s) = cand.as_str() {
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

/// Extract a list of workspace (team) IDs from the
/// `CLICKUP_GET_AUTHORIZED_TEAMS_WORKSPACES` response.
///
/// ClickUp returns `{"teams": [{"id": "...", "name": "..."}, …]}`. We
/// keep the IDs as strings — `CLICKUP_GET_FILTERED_TEAM_TASKS` requires
/// a `team_id` (string) argument.
pub fn extract_workspace_ids(data: &Value) -> Vec<String> {
    let candidates = [
        data.pointer("/teams"),
        data.pointer("/data/teams"),
        data.pointer("/workspaces"),
        data.pointer("/data/workspaces"),
    ];
    for cand in candidates.into_iter().flatten() {
        if let Some(arr) = cand.as_array() {
            return arr
                .iter()
                .filter_map(|t| pick_str(t, &["id", "team_id", "workspace_id"]))
                .collect();
        }
    }
    Vec::new()
}

#[cfg(test)]
#[path = "clickup_tests.rs"]
mod tests;
