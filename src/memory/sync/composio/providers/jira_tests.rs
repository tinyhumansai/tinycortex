//! Unit tests for the Jira Composio sync pipeline.

use async_trait::async_trait;
use serde_json::json;

use super::*;
use crate::memory::config::{ComposioMode, ComposioSyncConfig, MemoryConfig};
use crate::memory::sync::composio::client::{ActionExecutor, ExecuteResponse};
use crate::memory::sync::composio::{ComposioClient, IncrementalSource, SyncItem, SyncScope};
use crate::memory::sync::state::SyncState;

/// Executor that must never be invoked: `document()` folds comments in from the
/// list payload and issues no follow-up action.
struct UnusedExecutor;

#[async_trait]
impl ActionExecutor for UnusedExecutor {
    async fn execute(
        &self,
        action: &str,
        _arguments: serde_json::Value,
        _connection_id: Option<&str>,
    ) -> anyhow::Result<ExecuteResponse> {
        panic!("Jira document() must not call the executor (action: {action})");
    }
}

fn test_composio_config() -> ComposioSyncConfig {
    ComposioSyncConfig {
        mode: ComposioMode::Direct,
        base_url: "https://composio.test".into(),
        api_key: None,
        bearer_token: None,
        entity_id: None,
    }
}

fn pipeline() -> JiraSyncPipeline {
    JiraSyncPipeline::new(ComposioClient::new(test_composio_config()), "jira-conn")
}

/// A representative single-page search response with an issue and folded-in
/// comment, mirroring `JIRA_SEARCH_FOR_ISSUES_USING_JQL`.
fn sample_page() -> serde_json::Value {
    json!({
        "data": {
            "startAt": 0,
            "maxResults": 50,
            "total": 1,
            "issues": [
                {
                    "id": "10001",
                    "key": "PROJ-7",
                    "fields": {
                        "summary": "Login button misaligned",
                        "updated": "2026-03-01T12:00:00.000+0000",
                        "comment": {
                            "comments": [
                                {"body": "Reproduced on Safari", "author": {"displayName": "Sam"}}
                            ]
                        }
                    }
                }
            ]
        }
    })
}

#[test]
fn toolkit_and_action_are_stable() {
    let pipeline = pipeline();
    assert_eq!(pipeline.toolkit(), "jira");
    assert_eq!(pipeline.action(), "JIRA_SEARCH_FOR_ISSUES_USING_JQL");
    assert_eq!(pipeline.id(), "composio:jira");
}

#[test]
fn extract_page_reads_issues_and_stops_at_last_offset_page() {
    let pipeline = pipeline();
    let fetched = pipeline.extract_page(&sample_page(), None);
    assert_eq!(fetched.items.len(), 1);
    assert_eq!(fetched.items[0]["key"], "PROJ-7");
    // startAt(0) + 1 item == total(1), so there is no further page.
    assert_eq!(fetched.next, None);
}

#[test]
fn extract_page_advances_offset_when_more_remain() {
    let pipeline = JiraSyncPipeline::new(ComposioClient::new(test_composio_config()), "jira-conn")
        .with_limits(20, 2);
    let data = json!({
        "data": {
            "startAt": 0,
            "maxResults": 2,
            "total": 5,
            "issues": [{"key": "PROJ-1"}, {"key": "PROJ-2"}]
        }
    });
    let fetched = pipeline.extract_page(&data, None);
    assert_eq!(fetched.items.len(), 2);
    assert_eq!(fetched.next.as_deref(), Some("2"));
}

#[test]
fn extract_page_follows_enhanced_next_page_token() {
    let pipeline = pipeline();
    let data = json!({
        "data": {
            "nextPageToken": "opaque-cursor-abc",
            "isLast": false,
            "issues": [{"key": "PROJ-9"}]
        }
    });
    let fetched = pipeline.extract_page(&data, None);
    assert_eq!(fetched.next.as_deref(), Some("opaque-cursor-abc"));
}

#[test]
fn extract_page_honors_is_last_flag() {
    let pipeline = pipeline();
    let data = json!({
        "data": {"isLast": true, "issues": [{"key": "PROJ-9"}]}
    });
    assert_eq!(pipeline.extract_page(&data, None).next, None);
}

#[test]
fn arguments_switch_between_offset_and_token_pagination() {
    let pipeline = pipeline();
    let config = MemoryConfig::new("/tmp/tinycortex-jira-test");
    let state = SyncState::new("jira", "jira-conn");
    let scope = SyncScope::flat();

    let numeric = pipeline.arguments(&scope, &config, &state, Some("50"));
    assert_eq!(numeric["startAt"], json!(50));
    assert!(numeric.get("nextPageToken").is_none());
    assert_eq!(numeric["jql"], "order by updated DESC");

    let token = pipeline.arguments(&scope, &config, &state, Some("opaque-cursor"));
    assert_eq!(token["nextPageToken"], "opaque-cursor");
    assert!(token.get("startAt").is_none());
}

#[test]
fn dedup_key_binds_id_to_updated_cursor() {
    let pipeline = pipeline();
    let item = &sample_page()["data"]["issues"][0];
    assert_eq!(
        pipeline.dedup_key(item).as_deref(),
        Some("PROJ-7@2026-03-01T12:00:00.000+0000")
    );
    assert_eq!(
        pipeline.sort_cursor(item).as_deref(),
        Some("2026-03-01T12:00:00.000+0000")
    );
}

#[tokio::test]
async fn document_uses_stable_key_as_document_id() {
    let pipeline = pipeline();
    let raw = sample_page()["data"]["issues"][0].clone();
    let item = SyncItem {
        dedup_key: "PROJ-7@2026-03-01T12:00:00.000+0000".into(),
        sort_cursor: Some("2026-03-01T12:00:00.000+0000".into()),
        raw,
    };
    let mut state = SyncState::new("jira", "jira-conn");
    let document = pipeline
        .document(
            &SyncScope::flat(),
            "jira-conn",
            item,
            &UnusedExecutor,
            &mut state,
        )
        .await
        .unwrap();

    // Stable, per-issue upsert key — no per-run suffix.
    assert_eq!(document.document_id, "jira:PROJ-7");
    assert_eq!(document.toolkit, "jira");
    assert_eq!(document.namespace_skill_id, "jira");
    assert_eq!(document.metadata["taint"], "external_sync");
    assert!(document.title.contains("Login button misaligned"));
    // Comments are folded into the document content.
    assert!(document.content.contains("Reproduced on Safari"));
}
