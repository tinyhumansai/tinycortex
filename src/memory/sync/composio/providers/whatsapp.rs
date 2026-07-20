//! Incremental WhatsApp synchronization through Composio.
//!
//! WhatsApp is a message-shaped source: a chat account exposes a flat stream of
//! messages that Composio pages through. The shape mirrors [`GmailSyncPipeline`]
//! (one action, cursor-paged, incremental by timestamp) rather than the
//! scope-per-container Slack pipeline — WhatsApp's fetch action already returns
//! messages across chats in a single stream.
//!
//! Dedupe is keyed on the provider's stable per-message id (WhatsApp `wamid`),
//! surfaced as `document_id = "whatsapp:<message_id>"`. Because the id is stable
//! across runs, re-syncing upserts the same document instead of duplicating it —
//! per-run request ids are never used as the dedupe key.
//!
//! [`GmailSyncPipeline`]: crate::memory::sync::composio::GmailSyncPipeline

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

/// Toolkit slug advertised by Composio for the WhatsApp integration.
const TOOLKIT: &str = "whatsapp";

/// Composio fetch action for WhatsApp chat messages (`<TOOLKIT>_<VERB>`).
const ACTION_FETCH_MESSAGES: &str = "WHATSAPP_FETCH_MESSAGES";

/// Incremental WhatsApp message sync over a single Composio connection.
pub struct WhatsappSyncPipeline {
    client: ComposioClient,
    connection_id: String,
    max_pages: usize,
    page_size: usize,
}

impl WhatsappSyncPipeline {
    /// Build a pipeline for `connection_id` with conservative default limits.
    pub fn new(client: ComposioClient, connection_id: impl Into<String>) -> Self {
        Self {
            client,
            connection_id: connection_id.into(),
            max_pages: 20,
            // WhatsApp message payloads are small (text + envelope metadata), so a
            // larger page than Gmail's full-payload cap stays well under Composio's
            // tool-response size limit.
            page_size: 100,
        }
    }

    /// Override the per-run page ceiling and per-request page size.
    pub fn with_limits(mut self, max_pages: usize, page_size: usize) -> Self {
        self.max_pages = max_pages.max(1);
        self.page_size = page_size.max(1);
        self
    }
}

#[async_trait]
impl SyncPipeline for WhatsappSyncPipeline {
    fn id(&self) -> &str {
        "composio:whatsapp"
    }

    fn kind(&self) -> SyncPipelineKind {
        SyncPipelineKind::Composio
    }

    async fn init(&self, _config: &MemoryConfig, _context: &SyncContext) -> anyhow::Result<()> {
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
impl IncrementalSource for WhatsappSyncPipeline {
    fn toolkit(&self) -> &'static str {
        TOOLKIT
    }

    fn action(&self) -> &'static str {
        ACTION_FETCH_MESSAGES
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
        let mut arguments = serde_json::json!({ "limit": self.page_size });
        if let Some(token) = page {
            arguments["cursor"] = serde_json::json!(token);
        } else if let Some(cursor) = state.cursor.as_deref() {
            arguments["after"] = serde_json::json!(cursor_to_seconds(cursor).unwrap_or_default());
        } else if let Some(days) = config.sync.budget.sync_depth_days {
            arguments["after"] = serde_json::json!((chrono::Utc::now()
                - chrono::Duration::days(days as i64))
            .timestamp());
        }
        arguments
    }

    fn extract_page(&self, data: &Value, _page: Option<&str>) -> PageFetch {
        let items = first_array(
            data,
            &[
                "/data/messages",
                "/messages",
                "/data/data/messages",
                "/data/items",
                "/items",
                "/data/data/items",
            ],
        );
        // Content-free: message counts only, never message bodies or phone numbers.
        tracing::debug!(
            toolkit = TOOLKIT,
            count = items.len(),
            "[sync:whatsapp] extracted message page"
        );
        PageFetch {
            items,
            next: next_cursor(data),
        }
    }

    fn dedup_key(&self, item: &Value) -> Option<String> {
        message_id(item)
    }

    fn sort_cursor(&self, item: &Value) -> Option<String> {
        message_timestamp(item)
    }

    async fn document(
        &self,
        _scope: &SyncScope,
        connection_id: &str,
        item: SyncItem,
        _executor: &dyn ActionExecutor,
        _state: &mut SyncState,
    ) -> anyhow::Result<SkillDocument> {
        // Stable id → stable `document_id` ("whatsapp:<message_id>") so re-syncs
        // upsert rather than duplicate. Falls back to the dedupe key the engine
        // already derived from `dedup_key`.
        let id = message_id(&item.raw).unwrap_or_else(|| item.dedup_key.clone());
        // Content-free debug: no body, no chat identifier.
        tracing::debug!(
            toolkit = TOOLKIT,
            "[sync:whatsapp] mapping message to memory document"
        );
        let title = message_title(&item.raw);
        let content = serde_json::to_string_pretty(&item.raw)?;
        Ok(document(
            TOOLKIT,
            connection_id,
            &id,
            title,
            content,
            item.raw,
        ))
    }
}

/// Stable per-message id (WhatsApp `wamid`), used as the dedupe/document key.
fn message_id(message: &Value) -> Option<String> {
    pick_str(
        message,
        &[
            "id",
            "data.id",
            "message_id",
            "messageId",
            "wamid",
            "wam_id",
        ],
    )
}

/// Message send/receive timestamp, used as the incremental sort cursor.
fn message_timestamp(message: &Value) -> Option<String> {
    pick_str(
        message,
        &[
            "timestamp",
            "data.timestamp",
            "created_at",
            "createdAt",
            "date",
        ],
    )
}

/// Human-readable title scoped to the chat, without leaking message content.
fn message_title(message: &Value) -> String {
    match pick_str(message, &["chat_id", "chatId", "from", "wa_id", "waId"]) {
        Some(chat) => format!("WhatsApp chat {chat}"),
        None => "WhatsApp message".to_owned(),
    }
}

/// Extract the next-page cursor from the WhatsApp/Composio pagination envelope.
fn next_cursor(data: &Value) -> Option<String> {
    [
        "/data/paging/cursors/after",
        "/paging/cursors/after",
        "/data/data/paging/cursors/after",
        "/data/next_cursor",
        "/next_cursor",
        "/data/nextCursor",
    ]
    .iter()
    .find_map(|path| data.pointer(path).and_then(Value::as_str))
    .map(str::trim)
    .filter(|token| !token.is_empty())
    .map(str::to_owned)
}

/// Normalize a persisted cursor (unix millis, unix seconds, or RFC3339) to
/// whole seconds for the provider's `after` filter.
fn cursor_to_seconds(cursor: &str) -> Option<i64> {
    let cursor = cursor.trim();
    if let Ok(value) = cursor.parse::<i64>() {
        // Heuristic: 13-digit values are milliseconds, 10-digit are seconds.
        return Some(if cursor.len() >= 13 {
            value / 1000
        } else {
            value
        });
    }
    chrono::DateTime::parse_from_rfc3339(cursor)
        .ok()
        .map(|date| date.timestamp())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::config::{ComposioMode, ComposioSyncConfig};

    fn pipeline() -> WhatsappSyncPipeline {
        let config = ComposioSyncConfig {
            mode: ComposioMode::Direct,
            base_url: "https://backend.composio.dev".into(),
            api_key: None,
            bearer_token: None,
            entity_id: None,
        };
        WhatsappSyncPipeline::new(ComposioClient::new(config), "conn-1")
    }

    struct NoopExecutor;

    #[async_trait]
    impl ActionExecutor for NoopExecutor {
        async fn execute(
            &self,
            _action: &str,
            _arguments: Value,
            _connection_id: Option<&str>,
        ) -> anyhow::Result<crate::memory::sync::composio::ExecuteResponse> {
            anyhow::bail!("no execution expected in this test")
        }
    }

    #[test]
    fn advertises_toolkit_and_fetch_action() {
        let pipeline = pipeline();
        assert_eq!(pipeline.toolkit(), "whatsapp");
        assert_eq!(pipeline.action(), "WHATSAPP_FETCH_MESSAGES");
        assert_eq!(pipeline.id(), "composio:whatsapp");
    }

    #[test]
    fn extract_page_reads_messages_and_cursor() {
        let pipeline = pipeline();
        let payload = serde_json::json!({
            "data": {
                "messages": [
                    {"id": "wamid.AAA", "timestamp": "1721000000", "from": "chat-1"},
                    {"id": "wamid.BBB", "timestamp": "1721000100", "from": "chat-1"}
                ],
                "paging": { "cursors": { "after": "CURSOR-2" } }
            }
        });

        let fetched = pipeline.extract_page(&payload, None);

        assert_eq!(fetched.items.len(), 2);
        assert_eq!(fetched.items[0]["id"], "wamid.AAA");
        assert_eq!(fetched.next.as_deref(), Some("CURSOR-2"));
        assert_eq!(
            pipeline.dedup_key(&fetched.items[0]).as_deref(),
            Some("wamid.AAA")
        );
        assert_eq!(
            pipeline.sort_cursor(&fetched.items[1]).as_deref(),
            Some("1721000100")
        );
    }

    #[test]
    fn extract_page_defaults_when_envelope_is_empty() {
        let pipeline = pipeline();
        let fetched = pipeline.extract_page(&serde_json::json!({}), None);
        assert!(fetched.items.is_empty());
        assert!(fetched.next.is_none());
    }

    #[tokio::test]
    async fn document_uses_stable_message_id_as_key() {
        let pipeline = pipeline();
        let raw = serde_json::json!({
            "id": "wamid.AAA",
            "timestamp": "1721000000",
            "from": "chat-1",
            "text": {"body": "hello"}
        });
        let mut state = SyncState::new("whatsapp", "conn-1");
        let item = SyncItem {
            dedup_key: "wamid.AAA".into(),
            sort_cursor: Some("1721000000".into()),
            raw: raw.clone(),
        };

        let doc = pipeline
            .document(
                &SyncScope::flat(),
                "conn-1",
                item,
                &NoopExecutor,
                &mut state,
            )
            .await
            .unwrap();

        // Stable key: identical across runs, so upserts dedupe.
        assert_eq!(doc.document_id, "whatsapp:wamid.AAA");
        assert_eq!(doc.toolkit, "whatsapp");
        assert_eq!(doc.namespace_skill_id, "whatsapp");
        assert_eq!(doc.metadata["taint"], "external_sync");
        assert_eq!(doc.title, "WhatsApp chat chat-1");
    }

    #[test]
    fn cursor_to_seconds_handles_millis_seconds_and_rfc3339() {
        assert_eq!(cursor_to_seconds("1721000000000"), Some(1721000000));
        assert_eq!(cursor_to_seconds("1721000000"), Some(1721000000));
        let expected = chrono::DateTime::parse_from_rfc3339("2024-07-15T00:00:00Z")
            .unwrap()
            .timestamp();
        assert_eq!(cursor_to_seconds("2024-07-15T00:00:00Z"), Some(expected));
        assert_eq!(cursor_to_seconds("not-a-timestamp"), None);
    }
}
