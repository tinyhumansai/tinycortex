use async_trait::async_trait;
use serde_json::Value;

use super::common::{document, first_array, pick_str};
use crate::memory::config::MemoryConfig;
use crate::memory::sync::composio::{
    run_incremental_sync, ActionExecutor, ComposioClient, IncrementalSource, PageFetch, SyncItem,
    SyncScope,
};
use crate::memory::sync::state::SyncState;
use crate::memory::sync::traits::{
    SkillDocument, SyncContext, SyncOutcome, SyncPipeline, SyncPipelineKind,
};

const ACTION_LIST_MESSAGES: &str = "OUTLOOK_LIST_MESSAGES";

/// Incremental Microsoft Outlook mail synchronization through Composio.
///
/// Outlook messages carry a stable `id` and a `receivedDateTime` timestamp, so
/// this follows the message-shaped pattern (`GmailSyncPipeline`): a single list
/// action ordered newest-first, a client-visible `receivedDateTime` cursor, and
/// content taken directly from the message payload with no secondary fetch.
pub struct OutlookSyncPipeline {
    client: ComposioClient,
    connection_id: String,
    max_pages: usize,
    page_size: usize,
}

impl OutlookSyncPipeline {
    pub fn new(client: ComposioClient, connection_id: impl Into<String>) -> Self {
        Self {
            client,
            connection_id: connection_id.into(),
            max_pages: 10,
            page_size: 25,
        }
    }

    pub fn with_limits(mut self, max_pages: usize, page_size: usize) -> Self {
        self.max_pages = max_pages.max(1);
        self.page_size = page_size.max(1);
        self
    }
}

#[async_trait]
impl SyncPipeline for OutlookSyncPipeline {
    fn id(&self) -> &str {
        "composio:outlook"
    }
    fn kind(&self) -> SyncPipelineKind {
        SyncPipelineKind::Composio
    }
    async fn init(&self, _: &MemoryConfig, _: &SyncContext) -> anyhow::Result<()> {
        Ok(())
    }
    async fn tick(
        &self,
        config: &MemoryConfig,
        context: &SyncContext,
    ) -> anyhow::Result<SyncOutcome> {
        run_incremental_sync(self, &self.client, &self.connection_id, config, context).await
    }
}

#[async_trait]
impl IncrementalSource for OutlookSyncPipeline {
    fn toolkit(&self) -> &'static str {
        "outlook"
    }
    fn action(&self) -> &'static str {
        ACTION_LIST_MESSAGES
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
        _: &SyncScope,
        config: &MemoryConfig,
        state: &SyncState,
        page: Option<&str>,
    ) -> Value {
        // Microsoft Graph list-messages params passed through Composio: `top`
        // bounds the page size, `orderby` sorts newest-first by receive time.
        let mut args = serde_json::json!({
            "top": self.page_size,
            "orderby": "receivedDateTime desc",
        });
        if let Some(page) = page {
            // Graph paginates via a `$skiptoken`; `extract_page` has already
            // reduced the `@odata.nextLink` URL to the bare token. The exact
            // Composio arg name for feeding it back is not fully certain — we
            // send `skip_token` (the Graph-native name), so a mislabel here
            // surfaces as a single-page fetch, not silent data loss.
            args["skip_token"] = serde_json::json!(page);
        }
        // Depth window: prefer the last-synced cursor over the configured
        // horizon (same precedence as the Gmail/Calendar pipelines). Graph
        // filters server-side via `$filter` on `receivedDateTime`.
        if let Some(cursor) = state.cursor.as_deref() {
            args["filter"] = serde_json::json!(format!("receivedDateTime ge {cursor}"));
        } else if let Some(days) = config.sync.budget.sync_depth_days {
            let horizon = (chrono::Utc::now() - chrono::Duration::days(days as i64)).to_rfc3339();
            args["filter"] = serde_json::json!(format!("receivedDateTime ge {horizon}"));
        }
        args
    }
    fn extract_page(&self, data: &Value, _: Option<&str>) -> PageFetch {
        PageFetch {
            items: first_array(
                data,
                &[
                    "/data/value",
                    "/value",
                    "/data/messages",
                    "/messages",
                    "/data/data/value",
                    "/data/items",
                    "/items",
                ],
            ),
            next: [
                "/data/@odata.nextLink",
                "/@odata.nextLink",
                "/data/nextPageToken",
                "/nextPageToken",
                "/data/skip_token",
                "/skip_token",
            ]
            .iter()
            .find_map(|path| data.pointer(path).and_then(Value::as_str))
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .map(normalize_skip_token),
        }
    }
    fn dedup_key(&self, item: &Value) -> Option<String> {
        let id = pick_str(item, &["id", "data.id", "messageId", "data.messageId"])?;
        Some(match self.sort_cursor(item) {
            Some(received) => format!("{id}@{received}"),
            None => id,
        })
    }
    fn sort_cursor(&self, item: &Value) -> Option<String> {
        // Only `receivedDateTime` — the same field the `$filter` depth window
        // keys on. A `lastModifiedDateTime` fallback would store a cursor in a
        // different field than the filter compares, so on the next sync the
        // `receivedDateTime ge <cursor>` window could skip valid messages.
        pick_str(
            item,
            &[
                "receivedDateTime",
                "data.receivedDateTime",
                "received_date_time",
            ],
        )
    }
    async fn document(
        &self,
        _: &SyncScope,
        connection_id: &str,
        item: SyncItem,
        _: &dyn ActionExecutor,
        _: &mut SyncState,
    ) -> anyhow::Result<SkillDocument> {
        let id = pick_str(&item.raw, &["id", "data.id", "messageId", "data.messageId"])
            .unwrap_or_else(|| item.dedup_key.clone());
        let title = pick_str(&item.raw, &["subject", "data.subject", "title"])
            .unwrap_or_else(|| format!("Outlook message {id}"));
        let content = serde_json::to_string_pretty(&item.raw)?;
        Ok(document(
            "outlook",
            connection_id,
            &id,
            title,
            content,
            item.raw,
        ))
    }
}

/// Reduce a Graph paging token to the bare `$skiptoken` value.
///
/// Graph returns `@odata.nextLink` as a full URL
/// (`https://graph.microsoft.com/v1.0/me/messages?$skiptoken=ABC...`). Feeding
/// that whole URL back as the paging arg would not resume pagination, so when
/// the token looks like a URL we extract just the `skiptoken` query value;
/// otherwise (Composio may already surface the bare token) we pass it through.
fn normalize_skip_token(token: &str) -> String {
    let lower = token.to_ascii_lowercase();
    if let Some(pos) = lower.find("skiptoken=") {
        let value = &token[pos + "skiptoken=".len()..];
        let end = value.find('&').unwrap_or(value.len());
        return value[..end].to_string();
    }
    token.to_string()
}
