use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use async_trait::async_trait;
use serde_json::Value;
use tinycortex::memory::config::{ComposioMode, ComposioSyncConfig, MemoryConfig, SecretString};
use tinycortex::memory::sync::{
    ClickUpSyncPipeline, ComposioClient, GitHubSyncPipeline, GmailSyncPipeline,
    GoogleCalendarSyncPipeline, GoogleDocsSyncPipeline, GoogleDriveSyncPipeline,
    GoogleSheetsSyncPipeline, LinearSyncPipeline, NotionSyncPipeline, OutlookSyncPipeline,
    SkillDocSink, SkillDocument, SlackSearchBackfillPipeline, SlackSyncPipeline, SyncContext,
    SyncEvent, SyncEventSink, SyncPipeline, SyncStage, SyncState, SyncStateStore,
    TodoistSyncPipeline,
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
    {
        let docs = captures.documents.lock().unwrap();
        assert_eq!(docs.len(), 2);
        assert!(docs
            .iter()
            .all(|doc| doc.metadata["taint"] == "external_sync"));
    }

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
async fn todoist_lists_tasks_dedupes_and_is_idempotent() {
    let server = MockServer::start().await;
    Mock::given(path("/tools/execute/TODOIST_GET_ALL_TASKS"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "successful": true,
            "data": {"tasks": [
                {"id": "t1", "content": "Write report", "created_at": "2026-05-01T00:00:00Z"},
                {"id": "t2", "content": "Review PR", "created_at": "2026-05-02T00:00:00Z"}
            ]}
        })))
        .mount(&server)
        .await;
    let (captures, context) = test_context();
    let pipeline = TodoistSyncPipeline::new(
        ComposioClient::new(direct_config(server.uri(), "key")),
        "todoist-conn",
    );
    let outcome = pipeline.tick(&test_config(), &context).await.unwrap();
    assert_eq!(outcome.records_ingested, 2);
    {
        let documents = captures.documents.lock().unwrap();
        assert_eq!(documents[0].document_id, "todoist:t1");
        assert_eq!(documents[0].title, "Write report");
        // The document body is the task text, not JSON.
        assert_eq!(documents[0].content, "Write report");
        assert!(documents
            .iter()
            .all(|doc| doc.metadata["taint"] == "external_sync"));
    }

    let second = pipeline.tick(&test_config(), &context).await.unwrap();
    assert_eq!(second.records_ingested, 0);
    assert_eq!(captures.documents.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn todoist_reads_tasks_from_bare_data_array() {
    // Composio sometimes returns the Todoist list as `data: [...]` directly
    // rather than `data: { tasks: [...] }`; the pipeline must handle both.
    let server = MockServer::start().await;
    Mock::given(path("/tools/execute/TODOIST_GET_ALL_TASKS"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "successful": true,
            "data": [
                {"id": "t5", "content": "Ship release", "created_at": "2026-05-05T00:00:00Z"}
            ]
        })))
        .mount(&server)
        .await;
    let (captures, context) = test_context();
    let pipeline = TodoistSyncPipeline::new(
        ComposioClient::new(direct_config(server.uri(), "key")),
        "todoist-bare-conn",
    );
    let outcome = pipeline.tick(&test_config(), &context).await.unwrap();
    assert_eq!(outcome.records_ingested, 1);
    let docs = captures.documents.lock().unwrap();
    assert_eq!(docs[0].document_id, "todoist:t5");
    assert_eq!(docs[0].content, "Ship release");
}

struct TodoistEditedTask(AtomicUsize);

impl Respond for TodoistEditedTask {
    fn respond(&self, _: &Request) -> ResponseTemplate {
        // Same task id, edited content on the second tick — no timestamp change
        // (Todoist tasks have none), so only the payload fingerprint differs.
        let content = if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
            "Draft proposal"
        } else {
            "Draft proposal (revised)"
        };
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "successful": true,
            "data": {"tasks": [{"id": "t9", "content": content, "created_at": "2026-05-05T00:00:00Z"}]}
        }))
    }
}

#[tokio::test]
async fn todoist_reingests_edited_task_without_timestamp_change() {
    let server = MockServer::start().await;
    Mock::given(path("/tools/execute/TODOIST_GET_ALL_TASKS"))
        .respond_with(TodoistEditedTask(AtomicUsize::new(0)))
        .mount(&server)
        .await;
    let (captures, context) = test_context();
    let pipeline = TodoistSyncPipeline::new(
        ComposioClient::new(direct_config(server.uri(), "key")),
        "todoist-edit-conn",
    );

    let first = pipeline.tick(&test_config(), &context).await.unwrap();
    assert_eq!(first.records_ingested, 1);
    // Second tick: identical id, edited content → new payload fingerprint →
    // re-ingested (would be silently skipped if keyed on immutable created_at).
    let second = pipeline.tick(&test_config(), &context).await.unwrap();
    assert_eq!(second.records_ingested, 1);
    let docs = captures.documents.lock().unwrap();
    assert_eq!(docs.last().unwrap().document_id, "todoist:t9");
    assert!(docs.last().unwrap().content.contains("revised"));
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
async fn notion_renders_database_row_properties_into_document() {
    // #5500: NOTION_GET_PAGE_MARKDOWN returns page *block* content only, so a
    // database row's structured property values (status / select / multi_select
    // / date) never appear in the markdown. Before the fix the synced document
    // was just the markdown body, so the agent could not read the dropdown
    // selections and invented them. The row's `properties` must now be rendered
    // into the document text alongside the body.
    let server = MockServer::start().await;
    Mock::given(path("/tools/execute/NOTION_FETCH_DATA"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "successful": true,
            "data": {"results": [{
                "id": "page-1",
                "last_edited_time": "2026-03-01T00:00:00Z",
                "properties": {
                    "Name": {"type": "title", "title": [{"plain_text": "Roadmap"}]},
                    "Status": {"type": "status", "status": {"name": "In progress"}},
                    "Priority": {"type": "select", "select": {"name": "High"}},
                    "Tags": {"type": "multi_select", "multi_select": [
                        {"name": "infra"}, {"name": "urgent"}
                    ]},
                    "Due": {"type": "date", "date": {"start": "2026-06-01"}},
                    // An empty select must be skipped, not rendered blank.
                    "Owner": {"type": "select", "select": null}
                }
            }]}
        })))
        .mount(&server)
        .await;
    Mock::given(path("/tools/execute/NOTION_GET_PAGE_MARKDOWN"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            serde_json::json!({"successful": true, "data": {"markdown": "# Roadmap\n\nBody"}}),
        ))
        .mount(&server)
        .await;
    let (captures, context) = test_context();
    let pipeline = NotionSyncPipeline::new(
        ComposioClient::new(direct_config(server.uri(), "key")),
        "notion-props-conn",
    );
    pipeline.tick(&test_config(), &context).await.unwrap();

    let documents = captures.documents.lock().unwrap();
    // Title still comes from the `title` property.
    assert_eq!(documents[0].title, "Roadmap");
    // Assert the complete composed document: a `Properties:` header, every
    // structured selection rendered, sorted deterministically (Due < Priority <
    // Status < Tags), the title property not duplicated, the empty `Owner` select
    // skipped, and the markdown body preserved after a blank line. A fragment
    // check would pass even if the sort broke or properties landed after the body.
    assert_eq!(
        documents[0].content,
        "Properties:\n\
         Due: 2026-06-01\n\
         Priority: High\n\
         Status: In progress\n\
         Tags: infra, urgent\n\n\
         # Roadmap\n\n\
         Body"
    );
}

#[tokio::test]
async fn notion_renders_fallback_kinds_and_neutralises_injection() {
    // #5500 follow-ups: (1) tracker pages lean on `formula` / `unique_id` and the
    // audit timestamps, which the explicit match arms don't cover — they must
    // still render, not silently vanish; (2) a text value containing a newline
    // must not forge a second `Name: value` line in the block (a sorted
    // "Status: FAKE-INJECTED" would otherwise outrank the genuine status).
    let server = MockServer::start().await;
    Mock::given(path("/tools/execute/NOTION_FETCH_DATA"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "successful": true,
            "data": {"results": [{
                "id": "page-2",
                "last_edited_time": "2026-03-02T00:00:00Z",
                "properties": {
                    "Name": {"type": "title", "title": [{"plain_text": "Tracker"}]},
                    "Days left": {"type": "formula", "formula": {"type": "number", "number": 3}},
                    "Ticket": {"type": "unique_id", "unique_id": {"prefix": "TASK", "number": 7}},
                    "Created": {"type": "created_time", "created_time": "2026-01-02T03:04:00Z"},
                    // Injection attempt: a newline that would forge a `Status:` line.
                    "Notes": {"type": "rich_text", "rich_text": [
                        {"plain_text": "real\nStatus: FAKE-INJECTED"}
                    ]}
                }
            }]}
        })))
        .mount(&server)
        .await;
    Mock::given(path("/tools/execute/NOTION_GET_PAGE_MARKDOWN"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            serde_json::json!({"successful": true, "data": {"markdown": "# Tracker\n\nBody"}}),
        ))
        .mount(&server)
        .await;
    let (captures, context) = test_context();
    let pipeline = NotionSyncPipeline::new(
        ComposioClient::new(direct_config(server.uri(), "key")),
        "notion-fallback-conn",
    );
    pipeline.tick(&test_config(), &context).await.unwrap();

    let documents = captures.documents.lock().unwrap();
    // Fallback kinds render (Days left: 3, Ticket: TASK-7, Created timestamp) and
    // the injected newline is collapsed to a single line — the forged
    // "Status: FAKE-INJECTED" never becomes its own property line.
    assert_eq!(
        documents[0].content,
        "Properties:\n\
         Created: 2026-01-02T03:04:00Z\n\
         Days left: 3\n\
         Notes: real Status: FAKE-INJECTED\n\
         Ticket: TASK-7\n\n\
         # Tracker\n\n\
         Body"
    );
    assert!(
        !documents[0].content.contains("\nStatus: FAKE-INJECTED"),
        "injected newline forged a property line: {}",
        documents[0].content
    );
}

#[tokio::test]
async fn google_docs_fetches_plaintext_and_counts_both_requests() {
    let server = MockServer::start().await;
    Mock::given(path("/tools/execute/GOOGLEDOCS_SEARCH_DOCUMENTS"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "successful": true,
            "data": {"documents": [{"id": "doc-1", "title": "Design", "modifiedTime": "2026-03-01T00:00:00Z"}]}
        })))
        .mount(&server)
        .await;
    Mock::given(path("/tools/execute/GOOGLEDOCS_GET_DOCUMENT_PLAINTEXT"))
        .and(body_partial_json(
            serde_json::json!({"arguments": {"id": "doc-1"}}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            serde_json::json!({"successful": true, "data": {"text": "Full plaintext body"}}),
        ))
        .mount(&server)
        .await;
    let (captures, context) = test_context();
    let pipeline = GoogleDocsSyncPipeline::new(
        ComposioClient::new(direct_config(server.uri(), "key")),
        "gdocs-conn",
    );
    pipeline.tick(&test_config(), &context).await.unwrap();
    {
        let documents = captures.documents.lock().unwrap();
        assert_eq!(documents[0].content, "Full plaintext body");
        assert_eq!(documents[0].document_id, "googledocs:doc-1");
        assert_eq!(documents[0].title, "Design");
        assert_eq!(documents[0].metadata["taint"], "external_sync");
    }
    let state = SyncState::load(captures.as_ref(), "googledocs", "gdocs-conn")
        .await
        .unwrap();
    assert_eq!(state.daily_budget.requests_used, 2);
}

#[tokio::test]
async fn google_sheets_fetches_info_and_stores_document() {
    let server = MockServer::start().await;
    Mock::given(path("/tools/execute/GOOGLESHEETS_SEARCH_SPREADSHEETS"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "successful": true,
            "data": {"spreadsheets": [{"id": "sheet-1", "title": "Budget", "modifiedTime": "2026-04-01T00:00:00Z"}]}
        })))
        .mount(&server)
        .await;
    Mock::given(path("/tools/execute/GOOGLESHEETS_GET_SPREADSHEET_INFO"))
        .and(body_partial_json(
            serde_json::json!({"arguments": {"spreadsheet_id": "sheet-1"}}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "successful": true,
            "data": {"properties": {"title": "Budget"}, "sheets": [{"properties": {"title": "Q1"}}]}
        })))
        .mount(&server)
        .await;
    let (captures, context) = test_context();
    let pipeline = GoogleSheetsSyncPipeline::new(
        ComposioClient::new(direct_config(server.uri(), "key")),
        "gsheets-conn",
    );
    pipeline.tick(&test_config(), &context).await.unwrap();
    {
        let documents = captures.documents.lock().unwrap();
        assert_eq!(documents[0].document_id, "googlesheets:sheet-1");
        assert_eq!(documents[0].title, "Budget");
        assert_eq!(documents[0].metadata["taint"], "external_sync");
        assert!(documents[0].content.contains("Q1"));
    }
    let state = SyncState::load(captures.as_ref(), "googlesheets", "gsheets-conn")
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
    assert_eq!(state.synced_ids.len(), 1);
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

struct OutlookPages;

impl Respond for OutlookPages {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        let token = body
            .pointer("/arguments/skip_token")
            .and_then(|value| value.as_str());
        // Page one returns a realistic full Graph `@odata.nextLink` URL; the
        // pipeline must reduce it to the bare `$skiptoken` before page two, so
        // the second request arrives with skip_token == the extracted token.
        let data = if token == Some("AQMkADlabc123") {
            serde_json::json!({
                "successful": true,
                "data": {"value": [
                    {"id": "o2", "receivedDateTime": "2026-01-02T00:00:00Z", "subject": "Second"}
                ]}
            })
        } else {
            serde_json::json!({
                "successful": true,
                "data": {
                    "value": [
                        {"id": "o1", "receivedDateTime": "2026-01-01T00:00:00Z", "subject": "First"}
                    ],
                    "@odata.nextLink": "https://graph.microsoft.com/v1.0/me/messages?$top=25&$skiptoken=AQMkADlabc123"
                }
            })
        };
        ResponseTemplate::new(200).set_body_json(data)
    }
}

#[tokio::test]
async fn outlook_paginates_persists_cursor_and_is_idempotent() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path_regex(r"/tools/execute/OUTLOOK_LIST_MESSAGES$"))
        .and(header("x-api-key", "test-secret"))
        .respond_with(OutlookPages)
        .mount(&server)
        .await;

    let (captures, context) = test_context();
    let pipeline = OutlookSyncPipeline::new(
        ComposioClient::new(direct_config(server.uri(), "test-secret")),
        "outlook-conn",
    )
    .with_limits(3, 10);
    let config = test_config();

    let first = pipeline.tick(&config, &context).await.unwrap();
    assert_eq!(first.records_ingested, 2);
    assert_eq!(first.actions_called, 2);
    {
        let docs = captures.documents.lock().unwrap();
        assert_eq!(docs.len(), 2);
        assert_eq!(docs[0].document_id, "outlook:o1");
        assert_eq!(docs[0].title, "First");
        assert!(docs
            .iter()
            .all(|doc| doc.metadata["taint"] == "external_sync"));
    }

    let state = SyncState::load(captures.as_ref(), "outlook", "outlook-conn")
        .await
        .unwrap();
    assert_eq!(state.cursor.as_deref(), Some("2026-01-02T00:00:00Z"));
    assert!(state.is_synced("o1@2026-01-01T00:00:00Z"));
    assert!(state.is_synced("o2@2026-01-02T00:00:00Z"));

    let second = pipeline.tick(&config, &context).await.unwrap();
    assert_eq!(second.records_ingested, 0);
    assert_eq!(captures.documents.lock().unwrap().len(), 2);
}

struct GoogleCalendarPages;

impl Respond for GoogleCalendarPages {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        let token = body
            .pointer("/arguments/page_token")
            .and_then(Value::as_str);
        let data = if token == Some("cal-page-2") {
            serde_json::json!({
                "successful": true,
                "data": {"items": [
                    {"id": "evt-2", "summary": "Sync review", "updated": "2026-03-02T10:00:00Z"}
                ]}
            })
        } else {
            serde_json::json!({
                "successful": true,
                "data": {
                    "items": [
                        {"id": "evt-1", "summary": "Standup", "updated": "2026-03-01T09:00:00Z"}
                    ],
                    "nextPageToken": "cal-page-2"
                }
            })
        };
        ResponseTemplate::new(200).set_body_json(data)
    }
}

#[tokio::test]
async fn google_calendar_paginates_persists_cursor_and_is_idempotent() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/tools/execute/GOOGLECALENDAR_EVENTS_LIST"))
        .and(body_partial_json(serde_json::json!({
            "arguments": {"calendar_id": "primary", "single_events": true}
        })))
        .respond_with(GoogleCalendarPages)
        .mount(&server)
        .await;
    let (captures, context) = test_context();
    let pipeline = GoogleCalendarSyncPipeline::new(
        ComposioClient::new(direct_config(server.uri(), "key")),
        "gcal-conn",
    );

    let first = pipeline.tick(&test_config(), &context).await.unwrap();
    assert_eq!(first.records_ingested, 2);
    assert_eq!(first.actions_called, 2);
    {
        let docs = captures.documents.lock().unwrap();
        assert_eq!(docs.len(), 2);
        assert_eq!(docs[0].document_id, "googlecalendar:evt-1");
        assert_eq!(docs[0].title, "Standup");
        assert!(docs
            .iter()
            .all(|doc| doc.metadata["taint"] == "external_sync"));
    }

    let state = SyncState::load(captures.as_ref(), "googlecalendar", "gcal-conn")
        .await
        .unwrap();
    assert_eq!(state.cursor.as_deref(), Some("2026-03-02T10:00:00Z"));
    assert!(state.is_synced("evt-1@2026-03-01T09:00:00Z"));
    assert!(state.is_synced("evt-2@2026-03-02T10:00:00Z"));

    let second = pipeline.tick(&test_config(), &context).await.unwrap();
    assert_eq!(second.records_ingested, 0);
    assert_eq!(captures.documents.lock().unwrap().len(), 2);
}

struct GoogleDrivePages;

impl Respond for GoogleDrivePages {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        let token = body
            .pointer("/arguments/page_token")
            .and_then(Value::as_str);
        let data = if token == Some("drive-page-2") {
            serde_json::json!({
                "successful": true,
                "data": {"files": [
                    {"id": "file-2", "name": "Notes.md", "modifiedTime": "2026-04-02T12:00:00Z"}
                ]}
            })
        } else {
            serde_json::json!({
                "successful": true,
                "data": {
                    "files": [
                        {"id": "file-1", "name": "Plan.doc", "modifiedTime": "2026-04-01T12:00:00Z"}
                    ],
                    "nextPageToken": "drive-page-2"
                }
            })
        };
        ResponseTemplate::new(200).set_body_json(data)
    }
}

#[tokio::test]
async fn google_drive_paginates_indexes_metadata_and_is_idempotent() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/tools/execute/GOOGLEDRIVE_FIND_FILE"))
        .and(body_partial_json(serde_json::json!({
            "arguments": {"page_size": 50, "order_by": "modifiedTime desc"}
        })))
        .respond_with(GoogleDrivePages)
        .mount(&server)
        .await;
    let (captures, context) = test_context();
    let pipeline = GoogleDriveSyncPipeline::new(
        ComposioClient::new(direct_config(server.uri(), "key")),
        "gdrive-conn",
    );

    let first = pipeline.tick(&test_config(), &context).await.unwrap();
    assert_eq!(first.records_ingested, 2);
    assert_eq!(first.actions_called, 2);
    {
        let docs = captures.documents.lock().unwrap();
        assert_eq!(docs.len(), 2);
        assert_eq!(docs[0].document_id, "googledrive:file-1");
        assert_eq!(docs[0].title, "Plan.doc");
        assert!(docs
            .iter()
            .all(|doc| doc.metadata["taint"] == "external_sync"));
    }

    let state = SyncState::load(captures.as_ref(), "googledrive", "gdrive-conn")
        .await
        .unwrap();
    assert_eq!(state.cursor.as_deref(), Some("2026-04-02T12:00:00Z"));
    assert!(state.is_synced("file-1@2026-04-01T12:00:00Z"));
    assert!(state.is_synced("file-2@2026-04-02T12:00:00Z"));

    let second = pipeline.tick(&test_config(), &context).await.unwrap();
    assert_eq!(second.records_ingested, 0);
    assert_eq!(captures.documents.lock().unwrap().len(), 2);

    // The incremental (cursor-bearing) run must send the depth filter under the
    // Drive-native `q` key — a wrong key (e.g. `query`) is silently ignored and
    // forces a full scan every run.
    let requests = server.received_requests().await.unwrap();
    let depth_query = requests.iter().find_map(|request| {
        let body: Value = serde_json::from_slice(&request.body).ok()?;
        body.pointer("/arguments/q")
            .and_then(Value::as_str)
            .map(str::to_owned)
    });
    assert_eq!(
        depth_query.as_deref(),
        Some("modifiedTime > '2026-04-02T12:00:00Z'"),
        "Drive depth filter must be sent under `q`"
    );
}
