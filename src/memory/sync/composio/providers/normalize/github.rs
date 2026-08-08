//! GitHub host normalization helpers — result extraction, identity helpers, and time utilities.
//!
//! GitHub's REST API (proxied through Composio) returns search results and
//! authenticated-user payloads in a small number of shapes. The functions here
//! walk the union of common Composio envelope variants so the provider stays
//! clean and branch-free.

use serde_json::Value;

use super::helpers::pick_str;

/// Walk the Composio response envelope for GitHub search issue results.
///
/// `GITHUB_SEARCH_ISSUES_AND_PULL_REQUESTS` wraps GitHub's `GET /search/issues` response, which
/// returns `{"total_count": N, "items": [...]}`. Composio may re-wrap this under
/// `data` or `data.data`; we probe each shape in order.
pub fn extract_issues(data: &Value) -> Vec<Value> {
    let candidates = [
        data.pointer("/data/items"),
        data.pointer("/items"),
        data.pointer("/data/data/items"),
        data.pointer("/data/results"),
        data.pointer("/results"),
    ];
    for cand in candidates.into_iter().flatten() {
        if let Some(arr) = cand.as_array() {
            return arr.clone();
        }
    }
    Vec::new()
}

/// Extract a stable, globally unique identifier for a GitHub issue or PR.
///
/// GitHub's internal `id` field is a large integer unique across all issues
/// and PRs on github.com. We convert it to a string for use as a sync key.
/// Falls back to composing from `html_url` path if `id` is absent.
pub fn extract_issue_id(issue: &Value) -> Option<String> {
    // Primary: numeric internal GitHub ID.
    if let Some(id) = issue.get("id").or_else(|| issue.pointer("/data/id")) {
        if let Some(n) = id.as_u64() {
            return Some(n.to_string());
        }
        if let Some(s) = id.as_str() {
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    // Fallback: parse owner/repo/number from html_url path segments.
    // URL shape: https://github.com/{owner}/{repo}/issues/{number}
    if let Some(url) = pick_str(issue, &["html_url", "data.html_url", "url", "data.url"]) {
        if let Some(slug) = github_url_to_slug(&url) {
            return Some(slug);
        }
    }
    None
}

/// Build a human-readable document title for a GitHub issue/PR.
///
/// Format: `GitHub: {owner}/{repo}#{number}: {title}`.
/// Falls back to just the title or a placeholder when fields are missing.
pub fn extract_issue_title(issue: &Value) -> Option<String> {
    let title = pick_str(issue, &["title", "data.title"])?;

    // Best-effort: extract owner/repo#N from html_url for the prefix.
    let prefix = pick_str(issue, &["html_url", "data.html_url"])
        .and_then(|url| github_url_to_slug(&url))
        .unwrap_or_default();

    if prefix.is_empty() {
        Some(title)
    } else {
        Some(format!("GitHub: {prefix}: {title}"))
    }
}

/// Parse `https://github.com/{owner}/{repo}/issues/{number}` (or `/pull/`)
/// into `"{owner}/{repo}#{number}"`. Returns `None` for unrecognised shapes.
fn github_url_to_slug(url: &str) -> Option<String> {
    let segs: Vec<&str> = url.trim_end_matches('/').split('/').collect();
    // Minimum: ["https:", "", "github.com", owner, repo, "issues", number]
    if segs.len() >= 7 {
        let number = segs[segs.len() - 1];
        let _kind = segs[segs.len() - 2]; // "issues" or "pull" — ignored
        let repo = segs[segs.len() - 3];
        let owner = segs[segs.len() - 4];
        if !owner.is_empty() && !repo.is_empty() && !number.is_empty() {
            return Some(format!("{owner}/{repo}#{number}"));
        }
    }
    None
}

/// Extract the `updated_at` ISO 8601 timestamp from a GitHub issue.
///
/// GitHub returns `updated_at` as `"2024-05-21T15:30:00Z"`. ISO 8601 strings
/// sort lexicographically, so we use them directly as the sync cursor.
pub fn extract_issue_updated_at(issue: &Value) -> Option<String> {
    pick_str(
        issue,
        &[
            "updated_at",
            "data.updated_at",
            "updatedAt",
            "data.updatedAt",
        ],
    )
}

/// Extract the authenticated user's login handle from a
/// `GITHUB_GET_THE_AUTHENTICATED_USER` response.
pub fn extract_user_login(data: &Value) -> Option<String> {
    pick_str(data, &["login", "data.login"])
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
#[path = "github_tests.rs"]
mod tests;
