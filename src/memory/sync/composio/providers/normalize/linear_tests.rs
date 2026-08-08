use super::*;
use serde_json::json;

// ── extract_issues ───────────────────────────────────────────────

#[test]
fn extract_issues_from_data_nodes() {
    let data = json!({ "data": { "nodes": [{"id": "i1"}, {"id": "i2"}] } });
    assert_eq!(extract_issues(&data).len(), 2);
}

#[test]
fn extract_issues_from_top_level_nodes() {
    let data = json!({ "nodes": [{"id": "i3"}] });
    assert_eq!(extract_issues(&data).len(), 1);
}

#[test]
fn extract_issues_from_data_issues_nodes() {
    let data =
        json!({ "data": { "issues": { "nodes": [{"id": "i4"}, {"id": "i5"}, {"id": "i6"}] } } });
    assert_eq!(extract_issues(&data).len(), 3);
}

#[test]
fn extract_issues_from_top_level_issues_nodes() {
    let data = json!({ "issues": { "nodes": [{"id": "i7"}] } });
    assert_eq!(extract_issues(&data).len(), 1);
}

#[test]
fn extract_issues_from_doubly_nested_issues_nodes() {
    let data =
        json!({ "data": { "data": { "issues": { "nodes": [{"id": "i8"}, {"id": "i9"}] } } } });
    assert_eq!(extract_issues(&data).len(), 2);
}

#[test]
fn extract_issues_from_results() {
    let data = json!({ "results": [{"id": "i7"}] });
    assert_eq!(extract_issues(&data).len(), 1);
}

#[test]
fn extract_issues_empty_when_missing() {
    let data = json!({ "foo": "bar" });
    assert!(extract_issues(&data).is_empty());
}

// ── extract_issue_title ──────────────────────────────────────────

#[test]
fn extract_issue_title_from_title_field() {
    let issue = json!({ "id": "i1", "title": "Fix the login bug" });
    assert_eq!(
        extract_issue_title(&issue),
        Some("Fix the login bug".into())
    );
}

#[test]
fn extract_issue_title_falls_back_to_wrapped_data() {
    let issue = json!({ "data": { "title": "Wrapped issue" } });
    assert_eq!(extract_issue_title(&issue), Some("Wrapped issue".into()));
}

#[test]
fn extract_issue_title_falls_back_to_identifier() {
    let issue = json!({ "identifier": "ENG-42" });
    assert_eq!(extract_issue_title(&issue), Some("ENG-42".into()));
}

// ── extract_issue_updated ────────────────────────────────────────

#[test]
fn extract_issue_updated_from_updated_at() {
    let issue = json!({ "updatedAt": "2026-03-01T12:00:00.000Z" });
    assert_eq!(
        extract_issue_updated(&issue),
        Some("2026-03-01T12:00:00.000Z".to_string())
    );
}

#[test]
fn extract_issue_updated_falls_back_to_snake_case() {
    let issue = json!({ "data": { "updated_at": "2026-01-15T08:30:00.000Z" } });
    assert_eq!(
        extract_issue_updated(&issue),
        Some("2026-01-15T08:30:00.000Z".to_string())
    );
}

// ── extract_viewer ───────────────────────────────────────────────

#[test]
fn extract_viewer_from_data_nodes() {
    let data = json!({ "data": { "nodes": [{ "id": "usr_1", "email": "a@b.com" }] } });
    let v = extract_viewer(&data).expect("should find viewer");
    assert_eq!(v["id"], "usr_1");
}

#[test]
fn extract_viewer_from_top_level_nodes() {
    let data = json!({ "nodes": [{ "id": "usr_2" }] });
    let v = extract_viewer(&data).expect("should find viewer");
    assert_eq!(v["id"], "usr_2");
}

#[test]
fn extract_viewer_fallback_direct_object() {
    let data = json!({ "id": "usr_direct", "name": "Direct User" });
    let v = extract_viewer(&data).expect("should return direct object");
    assert_eq!(v["id"], "usr_direct");
}

#[test]
fn extract_viewer_returns_none_when_absent() {
    let data = json!({ "foo": "bar" });
    assert!(extract_viewer(&data).is_none());
}

// ── extract_pagination_cursor ────────────────────────────────────

#[test]
fn extract_pagination_cursor_returns_cursor_when_has_next_page() {
    let data = json!({
        "data": {
            "pageInfo": {
                "hasNextPage": true,
                "endCursor": "cursor_abc"
            }
        }
    });
    assert_eq!(
        extract_pagination_cursor(&data),
        Some("cursor_abc".to_string())
    );
}

#[test]
fn extract_pagination_cursor_returns_none_when_last_page() {
    let data = json!({
        "pageInfo": {
            "hasNextPage": false,
            "endCursor": "cursor_xyz"
        }
    });
    assert!(extract_pagination_cursor(&data).is_none());
}

#[test]
fn extract_pagination_cursor_from_doubly_nested_issues() {
    // The same `data.data.issues` shape `extract_issues` reads must also
    // expose its pageInfo cursor, or a doubly-nested payload never pages.
    let data = json!({
        "data": {
            "data": {
                "issues": {
                    "pageInfo": {
                        "hasNextPage": true,
                        "endCursor": "cursor_issue_2"
                    }
                }
            }
        }
    });
    assert_eq!(
        extract_pagination_cursor(&data),
        Some("cursor_issue_2".to_string())
    );
}

#[test]
fn extract_pagination_cursor_returns_none_when_absent() {
    let data = json!({ "nodes": [{"id": "i1"}] });
    assert!(extract_pagination_cursor(&data).is_none());
}

// ── now_ms ───────────────────────────────────────────────────────

#[test]
fn now_ms_returns_nonzero() {
    assert!(now_ms() > 0);
}
