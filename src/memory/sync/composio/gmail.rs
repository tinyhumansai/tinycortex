//! Incremental Gmail synchronization through Composio.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;

use super::client::{ActionExecutor, ComposioClient};
use super::orchestrator::{
    run_incremental_sync, IncrementalSource, PageFetch, SyncItem, SyncScope,
};
use crate::memory::config::MemoryConfig;
use crate::memory::ingest::canonicalize::email::{self, EmailMessage, EmailThread};
use crate::memory::ingest::canonicalize::email_clean;
use crate::memory::sync::state::SyncState;
use crate::memory::sync::traits::{
    SkillDocument, SyncContext, SyncOutcome, SyncPipeline, SyncPipelineKind,
};

const ACTION_FETCH_EMAILS: &str = "GMAIL_FETCH_EMAILS";

pub struct GmailSyncPipeline {
    executor: Arc<dyn ActionExecutor>,
    connection_id: String,
    max_pages: usize,
    page_size: usize,
    query_override: Option<String>,
}

impl GmailSyncPipeline {
    /// Sync through a plain Composio client.
    pub fn new(client: ComposioClient, connection_id: impl Into<String>) -> Self {
        Self::with_executor(Arc::new(client), connection_id)
    }

    /// Sync through a caller-supplied executor.
    ///
    /// The seam exists for host-side response reshaping: the Gmail envelope
    /// rewrite (verbose MIME payload → one slim record per message, body
    /// pre-rendered into `markdown`) lives in the host, above this crate, so
    /// wrapping the executor is the only way it can reach the fetched page
    /// before [`document`](SyncPipeline) turns it into a stored document.
    pub fn with_executor(
        executor: Arc<dyn ActionExecutor>,
        connection_id: impl Into<String>,
    ) -> Self {
        Self {
            executor,
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
        run_incremental_sync(
            self,
            self.executor.as_ref(),
            &self.connection_id,
            _config,
            context,
        )
        .await
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
            content: canonical_markdown(&item.raw, &id),
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

/// Render one Gmail message as canonical Markdown — the same shape the memory
/// tree ingests — rather than the provider's raw JSON.
///
/// Storing `to_string_pretty(&item.raw)` puts a MIME tree, `Received:` headers
/// and base64 part bodies into the document: the literal words of the mail are
/// either absent or split mid-token by the chunker, so recall can never match
/// them. Routing through [`email::canonicalise`] reuses the canonicaliser the
/// tree already uses — headers as a small block, body through
/// `email_clean::clean_body` (reply chains and footer boilerplate stripped).
fn canonical_markdown(message: &Value, id: &str) -> String {
    let thread = email_thread(message, id);
    match email::canonicalise(&format!("gmail:{id}"), "", &[], thread) {
        Ok(Some(canonical)) => canonical.markdown,
        // The thread built here always holds exactly one message, so an empty
        // thread (`None`) and `Err` are unreachable in practice. Degrade to the
        // bare body rather than dropping the message out of memory.
        _ => message_body(message),
    }
}

/// Adapt one provider message into the canonicaliser's input shape. A Gmail
/// sync item is a single message, so the thread wraps exactly one.
fn email_thread(message: &Value, id: &str) -> EmailThread {
    let subject = message_title(message);
    EmailThread {
        provider: "gmail".into(),
        thread_subject: subject.clone(),
        messages: vec![EmailMessage {
            from: message_sender(message),
            to: message_recipients(message),
            cc: Vec::new(),
            subject,
            sent_at: message_sent_at(message),
            body: message_body(message),
            source_ref: Some(format!("gmail:{id}")),
            list_unsubscribe: None,
        }],
    }
}

/// Body text for one message, best rendering first.
///
/// `markdown` is what the Gmail response reshaper pins onto each message (HTML
/// stripped, URLs shortened, footers removed). `messageText` is the provider's
/// own plain-text rendering, used when the reshape did not run. `snippet` is a
/// last resort: truncated, but real prose — unlike the raw payload.
fn message_body(message: &Value) -> String {
    ["markdown", "markdownFormatted", "messageText", "snippet"]
        .iter()
        .find_map(|key| nonempty_str(message, key))
        .unwrap_or_default()
}

/// Sender header, rendered as `From:` and used by the canonicaliser as the
/// participant key.
fn message_sender(message: &Value) -> String {
    ["from", "sender"]
        .iter()
        .find_map(|key| nonempty_str(message, key))
        .unwrap_or_else(|| "unknown".to_owned())
}

/// Recipients arrive as one comma-joined header string (some responses use an
/// array); split them so the canonicaliser can render a `To:` line.
fn message_recipients(message: &Value) -> Vec<String> {
    match message.get("to") {
        Some(Value::String(header)) => header
            .split(',')
            .map(str::trim)
            .filter(|address| !address.is_empty())
            .map(str::to_owned)
            .collect(),
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|address| !address.is_empty())
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

/// Send time, preferring the canonicaliser's own `Value`-level date parser (it
/// already knows `date`, `internalDate`, and epoch-ms-as-string) and falling
/// back to the sync cursor. The epoch is the last resort because it is
/// *deterministic*: a message the provider dated with nothing must not rewrite
/// its own content — and so re-chunk and re-embed — on every sync.
fn message_sent_at(message: &Value) -> DateTime<Utc> {
    email_clean::parse_message_date(message)
        .or_else(|| {
            item_cursor(message)
                .as_deref()
                .and_then(cursor_to_seconds)
                .and_then(|seconds| DateTime::from_timestamp(seconds, 0))
        })
        .unwrap_or_else(|| DateTime::from_timestamp(0, 0).expect("epoch is a valid timestamp"))
}

/// Read `key` as a trimmed, non-empty string. Unlike a plain `get(..).as_str()`
/// chain over a candidate list, a present-but-blank field falls through to the
/// next candidate instead of ending the search.
fn nonempty_str(message: &Value, key: &str) -> Option<String> {
    message
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn cursor_to_seconds(cursor: &str) -> Option<i64> {
    if let Ok(milliseconds) = cursor.trim().parse::<i64>() {
        return Some(milliseconds / 1000);
    }
    chrono::DateTime::parse_from_rfc3339(cursor)
        .ok()
        .map(|date| date.timestamp())
}

#[cfg(test)]
#[path = "gmail_tests.rs"]
mod tests;
