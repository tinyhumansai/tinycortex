use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use async_trait::async_trait;
use serde_json::Value;
use tinycortex::memory::config::{ComposioMode, ComposioSyncConfig, MemoryConfig, SecretString};
use tinycortex::memory::sync::{
    ClickUpSyncPipeline, ComposioClient, GitHubSyncPipeline, GmailSyncPipeline, LinearSyncPipeline,
    NotionSyncPipeline, SkillDocSink, SkillDocument, SlackSearchBackfillPipeline,
    SlackSyncPipeline, SyncContext, SyncEvent, SyncEventSink, SyncPipeline, SyncStage, SyncState,
    SyncStateStore,
};
use wiremock::matchers::{body_partial_json, header, method, path, path_regex};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

#[derive(Default)]
struct Captures {
    documents: Mutex<Vec<SkillDocument>>,
    events: Mutex<Vec<SyncEvent>>,
    state: Mutex<HashMap<String, serde_json::Value>>,
}

#[async_trait]
impl SkillDocSink for Captures {
    async fn store(&self, document: SkillDocument) -> anyhow::Result<()> {
        self.documents.lock().unwrap().push(document);
        Ok(())
    }

    async fn delete(&self, namespace_skill_id: &str, document_id: &str) -> anyhow::Result<()> {
        self.documents.lock().unwrap().retain(|document| {
            document.namespace_skill_id != namespace_skill_id || document.document_id != document_id
        });
        Ok(())
    }
}

#[async_trait]
impl SyncEventSink for Captures {
    async fn emit(&self, event: SyncEvent) -> anyhow::Result<()> {
        self.events.lock().unwrap().push(event);
        Ok(())
    }
}

#[async_trait]
impl SyncStateStore for Captures {
    async fn get(&self, namespace: &str, key: &str) -> anyhow::Result<Option<serde_json::Value>> {
        Ok(self
            .state
            .lock()
            .unwrap()
            .get(&format!("{namespace}:{key}"))
            .cloned())
    }

    async fn set(
        &self,
        namespace: &str,
        key: &str,
        value: &serde_json::Value,
    ) -> anyhow::Result<()> {
        self.state
            .lock()
            .unwrap()
            .insert(format!("{namespace}:{key}"), value.clone());
        Ok(())
    }
}

struct GmailPages;

impl Respond for GmailPages {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
        let token = body
            .pointer("/arguments/page_token")
            .and_then(|v| v.as_str());
        let data = if token == Some("page-2") {
            serde_json::json!({
                "successful": true,
                "data": {"messages": [
                    {"id": "m2", "internalDate": "1700000002000", "subject": "Second"}
                ]}
            })
        } else {
            serde_json::json!({
                "successful": true,
                "data": {
                    "messages": [
                        {"id": "m1", "internalDate": "1700000001000", "subject": "First"}
                    ],
                    "nextPageToken": "page-2"
                }
            })
        };
        ResponseTemplate::new(200).set_body_json(data)
    }
}

fn direct_config(base_url: String, key: &str) -> ComposioSyncConfig {
    ComposioSyncConfig {
        mode: ComposioMode::Direct,
        base_url,
        api_key: Some(SecretString::new(key)),
        bearer_token: None,
        entity_id: Some("entity-1".into()),
    }
}

fn test_context() -> (std::sync::Arc<Captures>, SyncContext) {
    let captures = std::sync::Arc::new(Captures::default());
    let context = SyncContext {
        events: captures.clone(),
        documents: captures.clone(),
        state: captures.clone(),
        local_documents: None,
        external_sources: None,
        summariser: None,
    };
    (captures, context)
}

fn test_config() -> MemoryConfig {
    MemoryConfig::new("/tmp/tinycortex-sync-test")
}

#[tokio::test]
async fn gmail_sync_paginates_persists_cursor_taint_and_is_idempotent() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path_regex(r"/tools/execute/GMAIL_FETCH_EMAILS$"))
        .and(header("x-api-key", "test-secret"))
        .respond_with(GmailPages)
        .mount(&server)
        .await;

    let captures = std::sync::Arc::new(Captures::default());
    let context = SyncContext {
        events: captures.clone(),
        documents: captures.clone(),
        state: captures.clone(),
        local_documents: None,
        external_sources: None,
        summariser: None,
    };
    let pipeline = GmailSyncPipeline::new(
        ComposioClient::new(direct_config(server.uri(), "test-secret")),
        "conn-1",
    )
    .with_limits(3, 10);
    let config = MemoryConfig::new("/tmp/tinycortex-sync-test");

    let first = pipeline.tick(&config, &context).await.unwrap();
    assert_eq!(first.records_ingested, 2);
    assert_eq!(first.actions_called, 2);
    let docs = captures.documents.lock().unwrap();
    assert_eq!(docs.len(), 2);
    assert!(docs
        .iter()
        .all(|doc| doc.metadata["taint"] == "external_sync"));
    drop(docs);

    let state = SyncState::load(captures.as_ref(), "gmail", "conn-1")
        .await
        .unwrap();
    assert_eq!(state.cursor.as_deref(), Some("1700000002000"));
    assert!(state.is_synced("m1"));
    assert!(state.is_synced("m2"));

    let second = pipeline.tick(&config, &context).await.unwrap();
    assert_eq!(second.records_ingested, 0);
    assert_eq!(captures.documents.lock().unwrap().len(), 2);
    let events = captures.events.lock().unwrap();
    assert!(events
        .iter()
        .any(|event| event.stage == SyncStage::Fetching));
    assert!(events
        .iter()
        .any(|event| event.stage == SyncStage::Completed));
}

#[tokio::test]
async fn max_items_stops_before_fetching_another_page() {
    let server = MockServer::start().await;
    Mock::given(path("/tools/execute/GMAIL_FETCH_EMAILS"))
        .respond_with(GmailPages)
        .expect(1)
        .mount(&server)
        .await;
    let (captures, context) = test_context();
    let pipeline = GmailSyncPipeline::new(
        ComposioClient::new(direct_config(server.uri(), "key")),
        "capped-conn",
    );
    let mut config = test_config();
    config.sync.budget.max_items = Some(1);
    let outcome = pipeline.tick(&config, &context).await.unwrap();
    assert_eq!(outcome.records_ingested, 1);
    assert_eq!(outcome.actions_called, 1);
    assert!(outcome.more_pending);
    let state = SyncState::load(captures.as_ref(), "gmail", "capped-conn")
        .await
        .unwrap();
    assert_eq!(
        state.cursor, None,
        "capped runs must not advance the cursor"
    );
}

#[tokio::test]
async fn direct_401_never_leaks_api_key() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(401).set_body_string("unauthorized test-secret"))
        .mount(&server)
        .await;
    let client = ComposioClient::new(direct_config(server.uri(), "test-secret"));
    let error = client
        .execute("GMAIL_FETCH_EMAILS", serde_json::json!({}), Some("conn"))
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("401"));
    assert!(!error.contains("test-secret"));
}

#[tokio::test]
async fn proxied_mode_uses_bearer_and_backend_execute_envelope() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/agent-integrations/composio/execute"))
        .and(header("authorization", "Bearer proxy-secret"))
        .and(body_partial_json(serde_json::json!({
            "tool": "GMAIL_FETCH_EMAILS"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "successful": true,
            "data": {"messages": []},
            "costUsd": 0.01
        })))
        .mount(&server)
        .await;
    let client = ComposioClient::new(ComposioSyncConfig {
        mode: ComposioMode::Proxied,
        base_url: server.uri(),
        api_key: None,
        bearer_token: Some(SecretString::new("proxy-secret")),
        entity_id: None,
    });
    let response = client
        .execute("GMAIL_FETCH_EMAILS", serde_json::json!({}), None)
        .await
        .unwrap();
    assert!(response.successful);
    assert_eq!(response.cost_usd, 0.01);
}

#[tokio::test]
async fn github_resolves_identity_and_stores_timestamped_issue() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/tools/execute/GITHUB_GET_THE_AUTHENTICATED_USER"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "successful": true, "data": {"login": "alice"}
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/tools/execute/GITHUB_SEARCH_ISSUES_AND_PULL_REQUESTS"))
        .and(body_partial_json(serde_json::json!({"arguments": {"q": "involves:alice"}})))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "successful": true,
            "data": {"items": [{"id": 42, "title": "Fix sync", "updated_at": "2026-01-02T03:04:05Z"}]}
        })))
        .expect(1)
        .mount(&server)
        .await;
    let (captures, context) = test_context();
    let pipeline = GitHubSyncPipeline::new(
        ComposioClient::new(direct_config(server.uri(), "key")),
        "github-conn",
    );
    let outcome = pipeline.tick(&test_config(), &context).await.unwrap();
    assert_eq!(outcome.records_ingested, 1);
    assert_eq!(outcome.actions_called, 2);
    assert_eq!(
        captures.documents.lock().unwrap()[0].document_id,
        "github:42"
    );
    let state = SyncState::load(captures.as_ref(), "github", "github-conn")
        .await
        .unwrap();
    assert!(state.is_synced("42@2026-01-02T03:04:05Z"));
}

#[tokio::test]
async fn slack_search_backfill_enriches_and_paginates_workspace_messages() {
    let server = MockServer::start().await;
    Mock::given(path("/tools/execute/SLACK_LIST_ALL_USERS"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "successful": true,
            "data": {"members": [{"id": "U1", "profile": {"display_name": "Alice"}}]}
        })))
        .mount(&server)
        .await;
    Mock::given(path("/tools/execute/SLACK_LIST_CONVERSATIONS"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "successful": true,
            "data": {"channels": [{"id": "C1", "name": "general", "is_private": false}]}
        })))
        .mount(&server)
        .await;
    Mock::given(path("/tools/execute/SLACK_SEARCH_MESSAGES"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "successful": true,
            "data": {"messages": {"matches": [{
                "ts": "1700000000.000001",
                "user": "U1",
                "text": "hello <@U1>",
                "channel": {"id": "C1"}
            }], "paging": {"pages": 1}}}
        })))
        .expect(1)
        .mount(&server)
        .await;
    let (captures, context) = test_context();
    let pipeline = SlackSearchBackfillPipeline::new(
        ComposioClient::new(direct_config(server.uri(), "key")),
        "slack-search-conn",
        30,
    );
    let outcome = pipeline.tick(&test_config(), &context).await.unwrap();
    assert_eq!(outcome.records_ingested, 1);
    assert_eq!(outcome.actions_called, 3);
    let documents = captures.documents.lock().unwrap();
    assert_eq!(documents[0].title, "Slack #general from Alice");
    assert!(documents[0].content.contains("Alice: hello @Alice"));
}

struct LinearPages;

impl Respond for LinearPages {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        let second = body.pointer("/arguments/after").is_some();
        let data = if second {
            serde_json::json!({"nodes": [{"id": "L-2", "title": "Second", "updatedAt": "2026-02-02T00:00:00Z"}], "pageInfo": {"hasNextPage": false}})
        } else {
            serde_json::json!({"nodes": [{"id": "L-1", "title": "First", "updatedAt": "2026-02-01T00:00:00Z"}], "pageInfo": {"hasNextPage": true, "endCursor": "linear-page-2"}})
        };
        ResponseTemplate::new(200)
            .set_body_json(serde_json::json!({"successful": true, "data": data}))
    }
}

#[tokio::test]
async fn linear_resolves_viewer_and_follows_graphql_cursor() {
    let server = MockServer::start().await;
    Mock::given(path("/tools/execute/LINEAR_LIST_LINEAR_USERS"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            serde_json::json!({"successful": true, "data": {"nodes": [{"id": "viewer-1"}]}}),
        ))
        .mount(&server)
        .await;
    Mock::given(path("/tools/execute/LINEAR_LIST_LINEAR_ISSUES"))
        .respond_with(LinearPages)
        .expect(2)
        .mount(&server)
        .await;
    let (captures, context) = test_context();
    let pipeline = LinearSyncPipeline::new(
        ComposioClient::new(direct_config(server.uri(), "key")),
        "linear-conn",
    );
    let outcome = pipeline.tick(&test_config(), &context).await.unwrap();
    assert_eq!(outcome.records_ingested, 2);
    assert_eq!(captures.documents.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn notion_fetches_markdown_and_counts_both_requests() {
    let server = MockServer::start().await;
    Mock::given(path("/tools/execute/NOTION_FETCH_DATA"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"successful": true, "data": {"results": [{"id": "page-1", "title": "Roadmap", "last_edited_time": "2026-03-01T00:00:00Z"}]}})))
        .mount(&server).await;
    Mock::given(path("/tools/execute/NOTION_GET_PAGE_MARKDOWN"))
        .and(body_partial_json(
            serde_json::json!({"arguments": {"page_id": "page-1"}}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            serde_json::json!({"successful": true, "data": {"markdown": "# Roadmap\n\nBody"}}),
        ))
        .mount(&server)
        .await;
    let (captures, context) = test_context();
    let pipeline = NotionSyncPipeline::new(
        ComposioClient::new(direct_config(server.uri(), "key")),
        "notion-conn",
    );
    pipeline.tick(&test_config(), &context).await.unwrap();
    assert_eq!(
        captures.documents.lock().unwrap()[0].content,
        "# Roadmap\n\nBody"
    );
    let state = SyncState::load(captures.as_ref(), "notion", "notion-conn")
        .await
        .unwrap();
    assert_eq!(state.daily_budget.requests_used, 2);
}

#[tokio::test]
async fn clickup_pages_each_workspace_with_resolved_user() {
    let server = MockServer::start().await;
    Mock::given(path("/tools/execute/CLICKUP_GET_AUTHORIZED_USER"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"successful": true, "data": {"user": {"id": 77}}}),
            ),
        )
        .mount(&server)
        .await;
    Mock::given(path("/tools/execute/CLICKUP_GET_AUTHORIZED_TEAMS_WORKSPACES"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"successful": true, "data": {"teams": [{"id": "ws-1"}, {"id": "ws-2"}]}})))
        .mount(&server).await;
    Mock::given(path("/tools/execute/CLICKUP_GET_FILTERED_TEAM_TASKS"))
        .and(body_partial_json(serde_json::json!({"arguments": {"assignees": ["77"]}})))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"successful": true, "data": {"tasks": [{"id": "task", "name": "Scoped", "date_updated": "1700000000000"}]}})))
        .expect(2).mount(&server).await;
    let (captures, context) = test_context();
    let pipeline = ClickUpSyncPipeline::new(
        ComposioClient::new(direct_config(server.uri(), "key")),
        "clickup-conn",
    );
    let outcome = pipeline.tick(&test_config(), &context).await.unwrap();
    assert_eq!(
        outcome.records_ingested, 1,
        "global dedup suppresses the duplicate task returned by both workspaces"
    );
    assert_eq!(
        captures.documents.lock().unwrap()[0].metadata["workspace_id"],
        "ws-1"
    );
}

struct SlackHistory;

impl Respond for SlackHistory {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        if body.pointer("/arguments/channel").and_then(Value::as_str) == Some("C_BAD") {
            ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"successful": false, "error": "channel unavailable"}),
            )
        } else {
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "successful": true,
                "data": {"messages": [{"ts": "1760000000.000123", "user": "U1", "text": "healthy channel"}]}
            }))
        }
    }
}

#[tokio::test]
async fn slack_holds_failed_channel_cursor_and_advances_healthy_channel() {
    let server = MockServer::start().await;
    Mock::given(path("/tools/execute/SLACK_LIST_ALL_USERS"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "successful": true,
            "data": {"members": [{"id": "U1", "profile": {"display_name": "Alice"}}]}
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/tools/execute/SLACK_LIST_CONVERSATIONS"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "successful": true,
            "data": {"channels": [
                {"id": "C_BAD", "name": "broken", "is_private": false},
                {"id": "C_GOOD", "name": "general", "is_private": false}
            ]}
        })))
        .mount(&server)
        .await;
    Mock::given(path("/tools/execute/SLACK_FETCH_CONVERSATION_HISTORY"))
        .respond_with(SlackHistory)
        .expect(2)
        .mount(&server)
        .await;
    let (captures, context) = test_context();
    let pipeline = SlackSyncPipeline::new(
        ComposioClient::new(direct_config(server.uri(), "key")),
        "slack-conn",
    );
    let outcome = pipeline.tick(&test_config(), &context).await.unwrap();
    assert_eq!(outcome.records_ingested, 1);
    assert_eq!(outcome.actions_called, 4);
    let state = SyncState::load(captures.as_ref(), "slack", "slack-conn")
        .await
        .unwrap();
    let cursors: Value = serde_json::from_str(state.cursor.as_deref().unwrap()).unwrap();
    assert!(cursors.get("C_BAD").is_none());
    assert_eq!(cursors["C_GOOD"], "1760000000.000123");
    assert!(state.synced_ids.is_empty());
    assert_eq!(
        captures.documents.lock().unwrap()[0].metadata["channel_id"],
        "C_GOOD"
    );
    assert!(captures.documents.lock().unwrap()[0]
        .content
        .contains("Alice"));
}

#[tokio::test]
async fn configured_depth_is_server_side_for_github_and_client_side_for_linear() {
    let server = MockServer::start().await;
    Mock::given(path("/tools/execute/GITHUB_GET_THE_AUTHENTICATED_USER"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"successful": true, "data": {"login": "alice"}})),
        )
        .mount(&server)
        .await;
    Mock::given(path(
        "/tools/execute/GITHUB_SEARCH_ISSUES_AND_PULL_REQUESTS",
    ))
    .respond_with(
        ResponseTemplate::new(200)
            .set_body_json(serde_json::json!({"successful": true, "data": {"items": []}})),
    )
    .mount(&server)
    .await;
    Mock::given(path("/tools/execute/LINEAR_LIST_LINEAR_USERS"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            serde_json::json!({"successful": true, "data": {"nodes": [{"id": "viewer"}]}}),
        ))
        .mount(&server)
        .await;
    let recent = (chrono::Utc::now() - chrono::Duration::days(1))
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();
    let old = (chrono::Utc::now() - chrono::Duration::days(60))
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();
    Mock::given(path("/tools/execute/LINEAR_LIST_LINEAR_ISSUES"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "successful": true,
            "data": {"nodes": [
                {"id": "recent", "title": "Recent", "updatedAt": recent},
                {"id": "old", "title": "Old", "updatedAt": old}
            ], "pageInfo": {"hasNextPage": false}}
        })))
        .mount(&server)
        .await;
    let mut config = test_config();
    config.sync.budget.sync_depth_days = Some(30);
    let (captures, context) = test_context();
    GitHubSyncPipeline::new(
        ComposioClient::new(direct_config(server.uri(), "key")),
        "github-depth",
    )
    .tick(&config, &context)
    .await
    .unwrap();
    let linear = LinearSyncPipeline::new(
        ComposioClient::new(direct_config(server.uri(), "key")),
        "linear-depth",
    )
    .tick(&config, &context)
    .await
    .unwrap();
    assert_eq!(linear.records_ingested, 1);
    assert_eq!(captures.documents.lock().unwrap().len(), 1);

    let requests = server.received_requests().await.unwrap();
    let request = requests
        .iter()
        .find(|request| {
            request
                .url
                .path()
                .ends_with("GITHUB_SEARCH_ISSUES_AND_PULL_REQUESTS")
        })
        .unwrap();
    let body: Value = serde_json::from_slice(&request.body).unwrap();
    let query = body
        .pointer("/arguments/q")
        .and_then(Value::as_str)
        .unwrap();
    assert!(query.starts_with("involves:alice updated:>"));
}

struct RetryThenGmail(AtomicUsize);

impl Respond for RetryThenGmail {
    fn respond(&self, _: &Request) -> ResponseTemplate {
        let attempt = self.0.fetch_add(1, Ordering::SeqCst);
        if attempt < 2 {
            ResponseTemplate::new(429)
        } else {
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "successful": true,
                "data": {"messages": [{"id": "retry-message", "internalDate": "1700000000000", "subject": "Recovered"}]}
            }))
        }
    }
}

#[tokio::test]
async fn transient_retries_are_counted_in_persisted_budget() {
    let server = MockServer::start().await;
    Mock::given(path("/tools/execute/GMAIL_FETCH_EMAILS"))
        .respond_with(RetryThenGmail(AtomicUsize::new(0)))
        .expect(3)
        .mount(&server)
        .await;
    let (captures, context) = test_context();
    let pipeline = GmailSyncPipeline::new(
        ComposioClient::new(direct_config(server.uri(), "key")),
        "retry-conn",
    );
    let outcome = pipeline.tick(&test_config(), &context).await.unwrap();
    assert_eq!(outcome.records_ingested, 1);
    assert_eq!(outcome.actions_called, 3);
    let state = SyncState::load(captures.as_ref(), "gmail", "retry-conn")
        .await
        .unwrap();
    assert_eq!(state.daily_budget.requests_used, 3);
}
