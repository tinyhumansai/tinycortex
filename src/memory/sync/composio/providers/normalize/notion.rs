//! Notion host normalization helpers — result extraction, pagination cursor,
//! page title extraction, and time utilities.

use serde_json::Value;

use super::helpers::pick_str;

/// Walk the Composio response envelope for Notion page results.
pub fn extract_results(data: &Value) -> Vec<Value> {
    let candidates = [
        data.pointer("/data/results"),
        data.pointer("/results"),
        data.pointer("/data/data/results"),
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

/// Extract the rendered page body markdown from a `NOTION_GET_PAGE_MARKDOWN`
/// response. Composio wraps action output in varying envelope shapes, so we
/// try the common locations tolerantly and return the first non-empty string.
/// Returns `None` if no markdown field is found (caller falls back to the
/// metadata-only body and logs the raw shape for diagnosis).
pub fn extract_page_markdown(data: &Value) -> Option<String> {
    const PATHS: &[&str] = &[
        "/markdown",
        "/data/markdown",
        "/data/response_data/markdown",
        "/response_data/markdown",
        "/data/content",
        "/content",
        "/data/markdown_content",
        "/markdown_content",
        "/text",
        "/data/text",
    ];
    for p in PATHS {
        if let Some(s) = data.pointer(p).and_then(Value::as_str) {
            if !s.trim().is_empty() {
                return Some(s.to_string());
            }
        }
    }
    None
}

/// Extract the Notion pagination cursor (for `start_cursor` on the
/// next request).
pub fn extract_notion_cursor(data: &Value) -> Option<String> {
    let candidates = [
        data.pointer("/data/next_cursor"),
        data.pointer("/next_cursor"),
        data.pointer("/data/data/next_cursor"),
    ];
    for cand in candidates.into_iter().flatten() {
        if let Some(s) = cand.as_str() {
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

/// Try to extract a human-readable title from a Notion page object.
///
/// Notion pages store the title in `properties.title` or
/// `properties.Name.title[0].plain_text`. We try several shapes.
pub fn extract_page_title(page: &Value) -> Option<String> {
    // Try the common `properties.title.title[0].plain_text` shape.
    let props = page
        .get("properties")
        .or_else(|| page.get("data")?.get("properties"));
    if let Some(props) = props {
        // Walk all properties looking for a "title" type field.
        if let Some(obj) = props.as_object() {
            for (_key, val) in obj {
                if val.get("type").and_then(Value::as_str) == Some("title") {
                    if let Some(arr) = val.get("title").and_then(Value::as_array) {
                        let text: String = arr
                            .iter()
                            .filter_map(|t| t.get("plain_text").and_then(Value::as_str))
                            .collect::<Vec<_>>()
                            .join("");
                        if !text.is_empty() {
                            return Some(text);
                        }
                    }
                }
            }
        }
    }

    // Fallback: top-level "title" field (some Composio shapes).
    pick_str(page, &["title", "data.title", "name", "data.name"])
}

pub fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
#[path = "notion_tests.rs"]
mod tests;
