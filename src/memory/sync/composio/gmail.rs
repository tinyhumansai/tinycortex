//! Incremental Gmail synchronization through Composio.

use async_trait::async_trait;
use serde_json::Value;

use super::client::{ActionExecutor, ComposioClient};
use super::orchestrator::{
    run_incremental_sync, IncrementalSource, PageFetch, SyncItem, SyncScope,
};
use crate::memory::config::MemoryConfig;
use crate::memory::sync::state::SyncState;
use crate::memory::sync::traits::{
    SkillDocument, SyncContext, SyncOutcome, SyncPipeline, SyncPipelineKind,
};

const ACTION_FETCH_EMAILS: &str = "GMAIL_FETCH_EMAILS";

pub struct GmailSyncPipeline {
    client: ComposioClient,
    connection_id: String,
    max_pages: usize,
    page_size: usize,
    query_override: Option<String>,
}

impl GmailSyncPipeline {
    pub fn new(client: ComposioClient, connection_id: impl Into<String>) -> Self {
        Self {
            client,
            connection_id: connection_id.into(),
            max_pages: 10,
            // Gmail fetches full message payloads (`include_payload: true`), so a
            // large page overflows Composio's tool-response size cap with HTTP
            // 413. 25 full messages/request stays comfortably under it; callers
            // needing more throughput can raise it via `with_limits`.
            page_size: 25,
            query_override: None,
        }
    }

    pub fn with_limits(mut self, max_pages: usize, page_size: usize) -> Self {
        self.max_pages = max_pages.max(1);
        self.page_size = page_size.max(1);
        self
    }

    pub fn with_query(mut self, query: impl Into<String>) -> Self {
        self.query_override = Some(query.into());
        self
    }
}

#[async_trait]
impl SyncPipeline for GmailSyncPipeline {
    fn id(&self) -> &str {
        "composio:gmail"
    }

    fn kind(&self) -> SyncPipelineKind {
        SyncPipelineKind::Composio
    }

    async fn init(&self, _config: &MemoryConfig, _context: &SyncContext) -> anyhow::Result<()> {
        Ok(())
    }

    async fn tick(
        &self,
        _config: &MemoryConfig,
        context: &SyncContext,
    ) -> anyhow::Result<SyncOutcome> {
        run_incremental_sync(self, &self.client, &self.connection_id, _config, context).await
    }
}

#[async_trait]
impl IncrementalSource for GmailSyncPipeline {
    fn toolkit(&self) -> &'static str {
        "gmail"
    }

    fn action(&self) -> &'static str {
        ACTION_FETCH_EMAILS
    }

    fn max_pages(&self) -> usize {
        self.max_pages
    }
    fn stop_on_empty_pending(&self) -> bool {
        true
    }

    fn server_side_depth(&self) -> bool {
        true
    }

    fn arguments(
        &self,
        _scope: &SyncScope,
        config: &MemoryConfig,
        state: &SyncState,
        page: Option<&str>,
    ) -> Value {
        let mut arguments = serde_json::json!({
            "max_results": self.page_size,
            "include_payload": true,
        });
        if let Some(token) = page {
            arguments["page_token"] = serde_json::json!(token);
        }
        if let Some(query) = self.query_override.as_deref() {
            arguments["query"] = Value::String(query.into());
        } else if let Some(cursor) = state.cursor.as_deref() {
            arguments["query"] = serde_json::json!(format!(
                "after:{}",
                cursor_to_seconds(cursor).unwrap_or_default()
            ));
        } else if let Some(days) = config.sync.budget.sync_depth_days {
            arguments["query"] = serde_json::json!(format!(
                "after:{}",
                (chrono::Utc::now() - chrono::Duration::days(days as i64)).timestamp()
            ));
        }
        arguments
    }

    fn extract_page(&self, data: &Value, _page: Option<&str>) -> PageFetch {
        PageFetch {
            items: extract_messages(data),
            next: extract_page_token(data),
        }
    }

    fn dedup_key(&self, item: &Value) -> Option<String> {
        item_id(item)
    }

    fn sort_cursor(&self, item: &Value) -> Option<String> {
        item_cursor(item)
    }

    async fn document(
        &self,
        _scope: &SyncScope,
        connection_id: &str,
        item: SyncItem,
        _executor: &dyn ActionExecutor,
        _state: &mut SyncState,
    ) -> anyhow::Result<SkillDocument> {
        let id = item_id(&item.raw).unwrap_or_else(|| item.dedup_key.clone());
        Ok(SkillDocument {
            namespace_skill_id: "gmail".into(),
            connection_id: connection_id.into(),
            document_id: format!("gmail:{id}"),
            title: message_title(&item.raw),
            content: serde_json::to_string_pretty(&item.raw)?,
            toolkit: "gmail".into(),
            metadata: serde_json::json!({
                "source": "composio-provider-incremental",
                "taint": "external_sync",
                "message_id": id,
            }),
        })
    }
}

fn extract_messages(data: &Value) -> Vec<Value> {
    [
        "/data/messages",
        "/messages",
        "/data/data/messages",
        "/data/items",
        "/items",
    ]
    .iter()
    .find_map(|path| data.pointer(path).and_then(Value::as_array))
    .cloned()
    .unwrap_or_default()
}

fn extract_page_token(data: &Value) -> Option<String> {
    [
        "/data/nextPageToken",
        "/nextPageToken",
        "/data/data/nextPageToken",
    ]
    .iter()
    .find_map(|path| data.pointer(path).and_then(Value::as_str))
    .map(str::trim)
    .filter(|token| !token.is_empty())
    .map(str::to_owned)
}

fn item_id(message: &Value) -> Option<String> {
    ["id", "messageId", "message_id"]
        .iter()
        .find_map(|key| message.get(key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
}

fn item_cursor(message: &Value) -> Option<String> {
    ["internalDate", "internal_date", "date"]
        .iter()
        .find_map(|key| message.get(key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|cursor| !cursor.is_empty())
        .map(str::to_owned)
}

fn message_title(message: &Value) -> String {
    ["subject", "title"]
        .iter()
        .find_map(|key| message.get(key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .unwrap_or("Gmail message")
        .to_owned()
}

fn cursor_to_seconds(cursor: &str) -> Option<i64> {
    if let Ok(milliseconds) = cursor.trim().parse::<i64>() {
        return Some(milliseconds / 1000);
    }
    chrono::DateTime::parse_from_rfc3339(cursor)
        .ok()
        .map(|date| date.timestamp())
}
