//! Incremental Telegram synchronization through Composio.
//!
//! Telegram is a message-shaped source (like Gmail and Slack): the unit of
//! ingestion is a single chat message. There is no bot API to enumerate a
//! user's chats, so the pipeline discovers active chats from a single
//! `TELEGRAM_GET_UPDATES` poll and then pulls each chat's history with
//! `TELEGRAM_GET_CHAT_HISTORY` — the same "list scopes, then fetch per scope"
//! shape as [`super::slack::SlackSyncPipeline`].
//!
//! Dedupe is stable: the document id is `telegram:<chat_id>:<message_id>`, so
//! re-syncing a chat upserts the same document instead of creating duplicates
//! (see the dedupe note on the tracking issue). All debug logging is
//! content-free — only counts and toolkit/connection identifiers are emitted,
//! never message text, chat titles, or sender names.

use std::collections::BTreeMap;

use async_trait::async_trait;
use serde_json::Value;

use super::common::{checked_execute, document, first_array, pick_str};
use crate::memory::config::MemoryConfig;
use crate::memory::sync::composio::{
    run_incremental_sync, ActionExecutor, ComposioClient, IncrementalSource, PageFetch, SyncItem,
    SyncScope,
};
use crate::memory::sync::state::SyncState;
use crate::memory::sync::traits::{
    SkillDocument, SyncContext, SyncOutcome, SyncPipeline, SyncPipelineKind,
};

/// Chat discovery: a single poll whose updates reveal which chats are active.
const ACTION_UPDATES: &str = "TELEGRAM_GET_UPDATES";
/// Message fetch: paginated chat history, filtered by `chat_id`.
const ACTION_HISTORY: &str = "TELEGRAM_GET_CHAT_HISTORY";

/// Candidate JSON pointers for the update array returned by `TELEGRAM_GET_UPDATES`.
const UPDATE_POINTERS: &[&str] = &[
    "/data/result",
    "/result",
    "/data/updates",
    "/updates",
    "/data/data/result",
];

/// Candidate JSON pointers for the message array returned by
/// `TELEGRAM_GET_CHAT_HISTORY`.
const MESSAGE_POINTERS: &[&str] = &[
    "/data/messages",
    "/messages",
    "/data/result",
    "/result",
    "/data/data/messages",
];

/// The message envelope inside an update object (plain, edited, or channel post).
const MESSAGE_KEYS: &[&str] = &[
    "message",
    "edited_message",
    "channel_post",
    "edited_channel_post",
];

pub struct TelegramSyncPipeline {
    client: ComposioClient,
    connection_id: String,
    max_pages: usize,
    page_size: usize,
}

impl TelegramSyncPipeline {
    pub fn new(client: ComposioClient, connection_id: impl Into<String>) -> Self {
        Self {
            client,
            connection_id: connection_id.into(),
            max_pages: 10,
            // `TELEGRAM_GET_CHAT_HISTORY` caps `limit` at 100 messages/request.
            page_size: 100,
        }
    }

    pub fn with_limits(mut self, max_pages: usize, page_size: usize) -> Self {
        self.max_pages = max_pages.max(1);
        self.page_size = page_size.clamp(1, 100);
        self
    }
}

#[async_trait]
impl SyncPipeline for TelegramSyncPipeline {
    fn id(&self) -> &str {
        "composio:telegram"
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
impl IncrementalSource for TelegramSyncPipeline {
    fn toolkit(&self) -> &'static str {
        "telegram"
    }
    fn action(&self) -> &'static str {
        ACTION_HISTORY
    }
    fn max_pages(&self) -> usize {
        self.max_pages
    }
    // Each chat is fenced independently: one bad chat_id must not abort the
    // whole account sync.
    fn tolerate_scope_errors(&self) -> bool {
        true
    }
    // The chat-history action has no server-side "since" filter, so incremental
    // convergence rides on the retained dedupe set: paging into a chat stops as
    // soon as a whole page is already-synced.
    fn per_scope_cursors(&self) -> bool {
        true
    }
    fn stop_on_empty_pending(&self) -> bool {
        true
    }
    // No date filter to push server-side; likewise nothing to prune client-side.
    fn server_side_depth(&self) -> bool {
        true
    }

    async fn scopes(
        &self,
        executor: &dyn ActionExecutor,
        connection_id: &str,
        state: &mut SyncState,
    ) -> anyhow::Result<Vec<SyncScope>> {
        let response = checked_execute(
            executor,
            ACTION_UPDATES,
            serde_json::json!({ "limit": 100 }),
            connection_id,
            state,
        )
        .await?;

        // Distinct chats, ordered by id for deterministic scope iteration.
        let mut chats: BTreeMap<String, String> = BTreeMap::new();
        for update in first_array(&response.data, UPDATE_POINTERS) {
            let Some(message) = MESSAGE_KEYS.iter().find_map(|key| update.get(*key)) else {
                continue;
            };
            let Some(chat) = message.get("chat") else {
                continue;
            };
            let Some(chat_id) = pick_str(chat, &["id"]) else {
                continue;
            };
            let label = chat_label(chat, &chat_id);
            chats.entry(chat_id).or_insert(label);
        }

        tracing::debug!(
            toolkit = "telegram",
            connection_id,
            chats = chats.len(),
            "[sync:telegram] discovered chats"
        );
        Ok(chats
            .into_iter()
            .map(|(id, label)| SyncScope::named(id, label))
            .collect())
    }

    fn arguments(
        &self,
        scope: &SyncScope,
        _: &MemoryConfig,
        _: &SyncState,
        page: Option<&str>,
    ) -> Value {
        let mut args = serde_json::json!({
            "chat_id": scope.id,
            "limit": self.page_size,
        });
        if let Some(offset) = page.and_then(|page| page.parse::<i64>().ok()) {
            args["offset"] = serde_json::json!(offset);
        }
        args
    }

    fn extract_page(&self, data: &Value, page: Option<&str>) -> PageFetch {
        let items = first_array(data, MESSAGE_POINTERS);
        let previous = page
            .and_then(|page| page.parse::<usize>().ok())
            .unwrap_or(0);
        // A full page implies more history remains; advance the offset by the
        // number of messages actually returned.
        let next = (items.len() >= self.page_size).then(|| (previous + items.len()).to_string());
        PageFetch { items, next }
    }

    fn dedup_key(&self, item: &Value) -> Option<String> {
        let message_id = message_id(item)?;
        Some(
            match pick_str(item, &["chat.id", "chat_id", "data.chat.id"]) {
                Some(chat_id) => format!("{chat_id}:{message_id}"),
                None => message_id,
            },
        )
    }

    fn sort_cursor(&self, item: &Value) -> Option<String> {
        pick_str(item, &["date", "data.date", "edit_date"])
    }

    async fn document(
        &self,
        scope: &SyncScope,
        connection_id: &str,
        item: SyncItem,
        _: &dyn ActionExecutor,
        _: &mut SyncState,
    ) -> anyhow::Result<SkillDocument> {
        let chat_id = pick_str(&item.raw, &["chat.id", "chat_id", "data.chat.id"])
            .unwrap_or_else(|| scope.id.clone());
        let message_id = message_id(&item.raw).unwrap_or_else(|| item.dedup_key.clone());
        let stable_id = format!("{chat_id}:{message_id}");

        let date = pick_str(&item.raw, &["date", "data.date"]).unwrap_or_default();
        let sender = pick_str(
            &item.raw,
            &[
                "from.username",
                "from.first_name",
                "from.id",
                "sender_chat.title",
            ],
        )
        .unwrap_or_else(|| "unknown".into());
        let text = pick_str(&item.raw, &["text", "caption", "data.text"]).unwrap_or_default();

        let title = format!("Telegram {} · message {message_id}", scope.label);
        let content = format!("[{date}] {sender}: {text}");
        let mut doc = document(
            "telegram",
            connection_id,
            &stable_id,
            title,
            content,
            item.raw,
        );
        // Stable per-chat collection scope: the upsert key stays constant across
        // re-syncs so a chat's messages never fan out into duplicate documents.
        doc.metadata["path_scope"] = Value::String(format!("telegram:chat:{chat_id}"));
        doc.metadata["chat_id"] = Value::String(chat_id);
        doc.metadata["chat_label"] = Value::String(scope.label.clone());
        Ok(doc)
    }
}

/// Human-readable label for a chat, falling back to its id.
fn chat_label(chat: &Value, chat_id: &str) -> String {
    pick_str(chat, &["title", "username", "first_name"])
        .unwrap_or_else(|| format!("chat {chat_id}"))
}

/// Stable per-chat message identifier.
fn message_id(message: &Value) -> Option<String> {
    pick_str(message, &["message_id", "id", "data.message_id", "data.id"])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::config::{ComposioMode, ComposioSyncConfig};

    struct NoExecutor;

    #[async_trait]
    impl ActionExecutor for NoExecutor {
        async fn execute(
            &self,
            _: &str,
            _: Value,
            _: Option<&str>,
        ) -> anyhow::Result<crate::memory::sync::composio::ExecuteResponse> {
            anyhow::bail!("executor not expected in this test")
        }
    }

    fn pipeline() -> TelegramSyncPipeline {
        let config = ComposioSyncConfig {
            mode: ComposioMode::Direct,
            base_url: "https://backend.composio.dev".into(),
            api_key: None,
            bearer_token: None,
            entity_id: None,
        };
        TelegramSyncPipeline::new(ComposioClient::new(config), "conn-1").with_limits(10, 2)
    }

    #[test]
    fn toolkit_and_action_are_telegram_specific() {
        let pipeline = pipeline();
        assert_eq!(pipeline.toolkit(), "telegram");
        assert_eq!(pipeline.action(), "TELEGRAM_GET_CHAT_HISTORY");
    }

    #[test]
    fn extract_page_reads_messages_and_offsets_on_full_page() {
        let pipeline = pipeline(); // page_size == 2
        let data = serde_json::json!({
            "data": {
                "messages": [
                    {"message_id": 10, "chat": {"id": -100}, "date": 1_700_000_000, "text": "hi"},
                    {"message_id": 11, "chat": {"id": -100}, "date": 1_700_000_100, "text": "there"}
                ]
            }
        });
        let page = pipeline.extract_page(&data, None);
        assert_eq!(page.items.len(), 2);
        // Full page (2 == page_size) -> next offset advances by the item count.
        assert_eq!(page.next.as_deref(), Some("2"));

        let short = serde_json::json!({ "data": { "messages": [
            {"message_id": 12, "chat": {"id": -100}, "date": 1_700_000_200, "text": "bye"}
        ] } });
        // Partial page -> no further pages.
        assert!(pipeline.extract_page(&short, Some("2")).next.is_none());
    }

    #[test]
    fn dedup_key_combines_chat_and_message_ids() {
        let pipeline = pipeline();
        let message =
            serde_json::json!({"message_id": 42, "chat": {"id": -100200}, "date": 1_700_000_000});
        assert_eq!(pipeline.dedup_key(&message).as_deref(), Some("-100200:42"));
        assert_eq!(
            pipeline.sort_cursor(&message).as_deref(),
            Some("1700000000")
        );
    }

    #[tokio::test]
    async fn document_has_stable_id_taint_and_scope() {
        let pipeline = pipeline();
        let scope = SyncScope::named("-100200", "Team Chat");
        let raw = serde_json::json!({
            "message_id": 42,
            "chat": {"id": -100200, "title": "Team Chat"},
            "from": {"username": "alice"},
            "date": 1_700_000_000,
            "text": "hello"
        });
        let item = SyncItem {
            dedup_key: "-100200:42".into(),
            sort_cursor: Some("1700000000".into()),
            raw,
        };
        let mut state = SyncState::new("telegram", "conn-1");
        let doc = pipeline
            .document(&scope, "conn-1", item, &NoExecutor, &mut state)
            .await
            .unwrap();

        // Stable dedupe key: telegram:<chat_id>:<message_id>.
        assert_eq!(doc.document_id, "telegram:-100200:42");
        assert_eq!(doc.namespace_skill_id, "telegram");
        assert_eq!(doc.toolkit, "telegram");
        assert_eq!(doc.metadata["taint"], "external_sync");
        assert_eq!(doc.metadata["path_scope"], "telegram:chat:-100200");
        assert_eq!(doc.metadata["chat_id"], "-100200");
    }
}
