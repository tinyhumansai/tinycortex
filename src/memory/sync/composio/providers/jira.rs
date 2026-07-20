//! Incremental Jira synchronization through Composio.
//!
//! Jira is a document-shaped source: each issue becomes one memory document,
//! with its comments folded in via the `comment` field so no per-issue follow-up
//! request is needed. The closest existing analogue is [`LinearSyncPipeline`] —
//! both page an issue list ordered by "updated" and dedupe on a stable item id.
//!
//! Fetches use `JIRA_SEARCH_FOR_ISSUES_USING_JQL`. The response envelope varies
//! between Jira Cloud's classic offset pagination (`startAt`/`maxResults`/
//! `total`) and the enhanced token pagination (`nextPageToken`/`isLast`); both
//! are handled by [`JiraSyncPipeline::next_page`].

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

const ACTION_SEARCH: &str = "JIRA_SEARCH_FOR_ISSUES_USING_JQL";

/// Fields requested per issue. `comment` folds the issue's comments into the
/// returned payload so each document carries its discussion without an extra
/// round-trip. Kept explicit (rather than `*all`) to bound response size.
const ISSUE_FIELDS: &[&str] = &[
    "summary",
    "description",
    "status",
    "assignee",
    "reporter",
    "priority",
    "issuetype",
    "project",
    "labels",
    "created",
    "updated",
    "comment",
];

/// Stable identifier candidates. The issue `key` (e.g. `PROJ-123`) is the
/// natural, human-readable handle; the numeric `id` is the immutable fallback.
/// Both are stable across syncs — never per-run — which keeps `document_id`
/// (`jira:<id>`) usable as the upsert key.
const ID_PATHS: &[&str] = &["key", "data.key", "id", "data.id"];

const UPDATED_PATHS: &[&str] = &[
    "fields.updated",
    "data.fields.updated",
    "updated",
    "data.updated",
];

pub struct JiraSyncPipeline {
    client: ComposioClient,
    connection_id: String,
    max_pages: usize,
    page_size: usize,
}

impl JiraSyncPipeline {
    pub fn new(client: ComposioClient, connection_id: impl Into<String>) -> Self {
        Self {
            client,
            connection_id: connection_id.into(),
            max_pages: 20,
            page_size: 50,
        }
    }

    pub fn with_limits(mut self, max_pages: usize, page_size: usize) -> Self {
        self.max_pages = max_pages.max(1);
        self.page_size = page_size.max(1);
        self
    }

    /// Resolve the token for the next page from a search response.
    ///
    /// Prefers the enhanced-search `nextPageToken` (opaque string). Falls back
    /// to classic offset pagination, encoding the next `startAt` as a numeric
    /// string — [`Self::arguments`] disambiguates the two by parsing the token
    /// as an integer (offset) or treating it as an opaque page token.
    fn next_page(&self, data: &Value, page_len: usize) -> Option<String> {
        if pointer_bool(data, &["/data/isLast", "/isLast", "/data/data/isLast"]) == Some(true) {
            return None;
        }
        if let Some(token) = pointer_str(
            data,
            &[
                "/data/nextPageToken",
                "/nextPageToken",
                "/data/data/nextPageToken",
            ],
        ) {
            return Some(token);
        }
        let start_at =
            pointer_i64(data, &["/data/startAt", "/startAt", "/data/data/startAt"]).unwrap_or(0);
        let next_start = start_at + page_len as i64;
        match pointer_i64(data, &["/data/total", "/total", "/data/data/total"]) {
            // Classic envelope advertises the full result count.
            Some(total) => (next_start < total).then(|| next_start.to_string()),
            // No total: keep paging only while pages come back full.
            None => (page_len >= self.page_size && page_len > 0).then(|| next_start.to_string()),
        }
    }
}

#[async_trait]
impl SyncPipeline for JiraSyncPipeline {
    fn id(&self) -> &str {
        "composio:jira"
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
impl IncrementalSource for JiraSyncPipeline {
    fn toolkit(&self) -> &'static str {
        "jira"
    }
    fn action(&self) -> &'static str {
        ACTION_SEARCH
    }
    fn max_pages(&self) -> usize {
        self.max_pages
    }
    fn arguments(
        &self,
        _: &SyncScope,
        _: &MemoryConfig,
        _: &SyncState,
        page: Option<&str>,
    ) -> Value {
        // "order by updated DESC" makes the engine's cursor-boundary detection
        // (newest-first) valid; the stored cursor short-circuits already-synced
        // issues on later runs.
        let mut args = serde_json::json!({
            "jql": "order by updated DESC",
            "maxResults": self.page_size,
            "fields": ISSUE_FIELDS,
        });
        if let Some(page) = page {
            match page.parse::<i64>() {
                Ok(start_at) => args["startAt"] = serde_json::json!(start_at),
                Err(_) => args["nextPageToken"] = serde_json::json!(page),
            }
        }
        args
    }
    fn extract_page(&self, data: &Value, _: Option<&str>) -> PageFetch {
        let items = first_array(
            data,
            &[
                "/data/issues",
                "/issues",
                "/data/data/issues",
                "/data/results",
                "/results",
                "/data/items",
                "/items",
            ],
        );
        let next = self.next_page(data, items.len());
        PageFetch { items, next }
    }
    fn dedup_key(&self, item: &Value) -> Option<String> {
        let id = pick_str(item, ID_PATHS)?;
        Some(match self.sort_cursor(item) {
            Some(updated) => format!("{id}@{updated}"),
            None => id,
        })
    }
    fn sort_cursor(&self, item: &Value) -> Option<String> {
        pick_str(item, UPDATED_PATHS)
    }
    async fn document(
        &self,
        _: &SyncScope,
        connection_id: &str,
        item: SyncItem,
        _: &dyn ActionExecutor,
        _: &mut SyncState,
    ) -> anyhow::Result<SkillDocument> {
        // Prefer the stable issue key/id; only fall back to the dedup key (which
        // embeds the cursor) if the payload is missing every identifier.
        let id = pick_str(&item.raw, ID_PATHS).unwrap_or_else(|| item.dedup_key.clone());
        let title = pick_str(
            &item.raw,
            &["fields.summary", "data.fields.summary", "summary"],
        )
        .map(|summary| format!("{id}: {summary}"))
        .unwrap_or_else(|| format!("Jira issue {id}"));
        let content = serde_json::to_string_pretty(&item.raw)?;
        // `document` sets `document_id = "jira:<id>"` and `metadata.taint =
        // "external_sync"`, keeping the upsert key stable across re-syncs.
        Ok(document(
            "jira",
            connection_id,
            &id,
            title,
            content,
            item.raw,
        ))
    }
}

fn pointer_str(data: &Value, pointers: &[&str]) -> Option<String> {
    pointers
        .iter()
        .find_map(|pointer| data.pointer(pointer).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn pointer_i64(data: &Value, pointers: &[&str]) -> Option<i64> {
    pointers
        .iter()
        .find_map(|pointer| data.pointer(pointer).and_then(Value::as_i64))
}

fn pointer_bool(data: &Value, pointers: &[&str]) -> Option<bool> {
    pointers
        .iter()
        .find_map(|pointer| data.pointer(pointer).and_then(Value::as_bool))
}

#[cfg(test)]
#[path = "jira_tests.rs"]
mod tests;
