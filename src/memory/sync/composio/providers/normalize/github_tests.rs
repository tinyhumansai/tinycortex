use super::*;
use serde_json::json;

#[test]
fn extract_issues_from_data_items() {
    let data = json!({ "data": { "items": [{"id": 1}] } });
    assert_eq!(extract_issues(&data).len(), 1);
}

#[test]
fn extract_issues_from_top_level_items() {
    let data = json!({ "items": [{"id": 1}, {"id": 2}] });
    assert_eq!(extract_issues(&data).len(), 2);
}

#[test]
fn extract_issues_empty_when_missing() {
    let data = json!({ "foo": "bar" });
    assert!(extract_issues(&data).is_empty());
}

#[test]
fn extract_issue_id_from_numeric_field() {
    let issue = json!({ "id": 123456789u64, "title": "Fix bug" });
    assert_eq!(extract_issue_id(&issue), Some("123456789".to_string()));
}

#[test]
fn extract_issue_id_from_wrapped_data() {
    let issue = json!({ "data": { "id": 99u64 } });
    assert_eq!(extract_issue_id(&issue), Some("99".to_string()));
}

#[test]
fn extract_issue_id_falls_back_to_html_url() {
    let issue = json!({
        "html_url": "https://github.com/owner/repo/issues/42"
    });
    assert_eq!(extract_issue_id(&issue), Some("owner/repo#42".to_string()));
}

#[test]
fn extract_issue_id_none_when_missing() {
    let issue = json!({ "title": "No ID here" });
    assert!(extract_issue_id(&issue).is_none());
}

#[test]
fn extract_issue_title_builds_prefixed_title() {
    let issue = json!({
        "id": 1u64,
        "title": "Fix race condition",
        "html_url": "https://github.com/acme/core/issues/99"
    });
    assert_eq!(
        extract_issue_title(&issue),
        Some("GitHub: acme/core#99: Fix race condition".to_string())
    );
}

#[test]
fn extract_issue_title_returns_raw_title_when_no_url() {
    let issue = json!({ "title": "Bare title" });
    assert_eq!(extract_issue_title(&issue), Some("Bare title".to_string()));
}

#[test]
fn extract_issue_title_none_when_missing() {
    let issue = json!({ "id": 1u64 });
    assert!(extract_issue_title(&issue).is_none());
}

#[test]
fn extract_issue_updated_at_from_top_level() {
    let issue = json!({ "updated_at": "2024-05-21T15:30:00Z" });
    assert_eq!(
        extract_issue_updated_at(&issue),
        Some("2024-05-21T15:30:00Z".to_string())
    );
}

#[test]
fn extract_issue_updated_at_from_data_wrapper() {
    let issue = json!({ "data": { "updated_at": "2023-01-01T00:00:00Z" } });
    assert_eq!(
        extract_issue_updated_at(&issue),
        Some("2023-01-01T00:00:00Z".to_string())
    );
}

#[test]
fn extract_issue_updated_at_none_when_missing() {
    let issue = json!({ "id": 1u64 });
    assert!(extract_issue_updated_at(&issue).is_none());
}

#[test]
fn extract_user_login_from_top_level() {
    let data = json!({ "login": "octocat" });
    assert_eq!(extract_user_login(&data), Some("octocat".to_string()));
}

#[test]
fn extract_user_login_from_data_wrapper() {
    let data = json!({ "data": { "login": "monalisa" } });
    assert_eq!(extract_user_login(&data), Some("monalisa".to_string()));
}

#[test]
fn extract_user_login_none_when_missing() {
    let data = json!({ "id": 1u64 });
    assert!(extract_user_login(&data).is_none());
}

#[test]
fn now_ms_returns_nonzero() {
    assert!(now_ms() > 0);
}
