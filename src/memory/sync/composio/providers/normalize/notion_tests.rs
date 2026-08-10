use super::*;
use serde_json::json;

#[test]
fn extract_results_from_data_results() {
    let data = json!({"data": {"results": [{"id": "page1"}]}});
    let results = extract_results(&data);
    assert_eq!(results.len(), 1);
}

#[test]
fn extract_page_markdown_reads_top_level_field() {
    // Matches the live GET_PAGE_MARKDOWN envelope observed empirically:
    // {id, markdown, object, request_id, truncated, unknown_block_ids}.
    let data = json!({
        "id": "p1",
        "markdown": "# Heading\n\nbody text",
        "object": "page",
        "truncated": false,
    });
    assert_eq!(
        extract_page_markdown(&data).as_deref(),
        Some("# Heading\n\nbody text")
    );
}

#[test]
fn extract_page_markdown_reads_nested_envelope() {
    let data = json!({ "data": { "markdown": "nested body" } });
    assert_eq!(extract_page_markdown(&data).as_deref(), Some("nested body"));
}

#[test]
fn extract_page_markdown_none_for_empty_or_missing() {
    // Empty markdown (a DB row with no body blocks) → None → metadata-only.
    assert_eq!(extract_page_markdown(&json!({ "markdown": "" })), None);
    assert_eq!(extract_page_markdown(&json!({ "markdown": "   " })), None);
    // No markdown field at all → None.
    assert_eq!(extract_page_markdown(&json!({ "id": "p1" })), None);
}

#[test]
fn extract_results_from_top_level() {
    let data = json!({"results": [{"id": "a"}, {"id": "b"}]});
    let results = extract_results(&data);
    assert_eq!(results.len(), 2);
}

#[test]
fn extract_results_from_data_items() {
    let data = json!({"data": {"items": [{"id": "x"}]}});
    let results = extract_results(&data);
    assert_eq!(results.len(), 1);
}

#[test]
fn extract_results_empty_when_no_match() {
    let data = json!({"foo": "bar"});
    assert!(extract_results(&data).is_empty());
}

#[test]
fn extract_notion_cursor_from_data() {
    let data = json!({"data": {"next_cursor": "cur123"}});
    assert_eq!(extract_notion_cursor(&data), Some("cur123".into()));
}

#[test]
fn extract_notion_cursor_from_top_level() {
    let data = json!({"next_cursor": "abc"});
    assert_eq!(extract_notion_cursor(&data), Some("abc".into()));
}

#[test]
fn extract_notion_cursor_none_when_empty() {
    let data = json!({"data": {"next_cursor": "  "}});
    assert_eq!(extract_notion_cursor(&data), None);
}

#[test]
fn extract_notion_cursor_none_when_missing() {
    assert_eq!(extract_notion_cursor(&json!({})), None);
}

#[test]
fn extract_page_title_from_properties_title_type() {
    let page = json!({
        "properties": {
            "Name": {
                "type": "title",
                "title": [{"plain_text": "Hello"}, {"plain_text": " World"}]
            }
        }
    });
    assert_eq!(extract_page_title(&page), Some("Hello World".into()));
}

#[test]
fn extract_page_title_from_nested_data_properties() {
    let page = json!({
        "data": {
            "properties": {
                "Title": {
                    "type": "title",
                    "title": [{"plain_text": "My Page"}]
                }
            }
        }
    });
    assert_eq!(extract_page_title(&page), Some("My Page".into()));
}

#[test]
fn extract_page_title_fallback_to_top_level_title() {
    let page = json!({"title": "Fallback Title"});
    assert_eq!(extract_page_title(&page), Some("Fallback Title".into()));
}

#[test]
fn extract_page_title_none_when_empty() {
    let page = json!({"properties": {"Name": {"type": "title", "title": []}}});
    // Empty title array means no text
    assert!(
        extract_page_title(&page).is_none() || extract_page_title(&page) == Some(String::new())
    );
}

#[test]
fn extract_page_title_none_when_no_title_field() {
    let page = json!({"id": "123"});
    assert!(extract_page_title(&page).is_none());
}

#[test]
fn now_ms_returns_nonzero() {
    assert!(now_ms() > 0);
}
