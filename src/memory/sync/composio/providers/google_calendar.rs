use async_trait::async_trait;
use serde_json::Value;

use super::common::{document, first_array, next_page_token, pick_str};
use crate::memory::config::MemoryConfig;
use crate::memory::sync::composio::{
    run_incremental_sync, ActionExecutor, ComposioClient, IncrementalSource, PageFetch, SyncItem,
    SyncScope,
};
use crate::memory::sync::state::SyncState;
use crate::memory::sync::traits::{
    SkillDocument, SyncContext, SyncOutcome, SyncPipeline, SyncPipelineKind,
};

const ACTION_EVENTS_LIST: &str = "GOOGLECALENDAR_EVENTS_LIST";

/// Incremental Google Calendar synchronization through Composio.
///
/// Events are self-contained records (stable id + `updated` timestamp), so this
/// follows the document-shaped pattern (`LinearSyncPipeline`) rather than the
/// message-shaped one: a single list action, client-visible `updated` cursor,
/// content taken directly from the event payload with no secondary fetch.
pub struct GoogleCalendarSyncPipeline {
    client: ComposioClient,
    connection_id: String,
    calendar_id: String,
    max_pages: usize,
    page_size: usize,
}

impl GoogleCalendarSyncPipeline {
    pub fn new(client: ComposioClient, connection_id: impl Into<String>) -> Self {
        Self {
            client,
            connection_id: connection_id.into(),
            calendar_id: "primary".into(),
            max_pages: 10,
            page_size: 50,
        }
    }

    pub fn with_limits(mut self, max_pages: usize, page_size: usize) -> Self {
        self.max_pages = max_pages.max(1);
        // Google Calendar caps `maxResults` at 2500; stay well under it.
        self.page_size = page_size.clamp(1, 2500);
        self
    }

    pub fn with_calendar(mut self, calendar_id: impl Into<String>) -> Self {
        self.calendar_id = calendar_id.into();
        self
    }
}

#[async_trait]
impl SyncPipeline for GoogleCalendarSyncPipeline {
    fn id(&self) -> &str {
        "composio:googlecalendar"
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
impl IncrementalSource for GoogleCalendarSyncPipeline {
    fn toolkit(&self) -> &'static str {
        "googlecalendar"
    }
    fn action(&self) -> &'static str {
        ACTION_EVENTS_LIST
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
        // `single_events` expands recurring series into concrete instances so
        // each carries a stable id; `order_by: "updated"` sorts ascending by
        // modification time (oldest change first), matching the `updated` cursor.
        let mut args = serde_json::json!({
            "calendar_id": self.calendar_id,
            "max_results": self.page_size,
            "single_events": true,
            "order_by": "updated",
        });
        if let Some(page) = page {
            args["page_token"] = serde_json::json!(page);
        }
        // The cursor is a modification time (the item `updated` field), so it
        // belongs on `updated_min` (last-modified lower bound) — NOT `time_min`,
        // which filters by event *start* time and would drop recently-edited
        // past events. `time_min` is only the start-time horizon for the first,
        // cursorless backfill; once a cursor exists, `updated_min` fully bounds
        // the incremental window.
        if let Some(cursor) = state.cursor.as_deref() {
            args["updated_min"] = serde_json::json!(cursor);
        } else if let Some(days) = config.sync.budget.sync_depth_days {
            args["time_min"] = serde_json::json!((chrono::Utc::now()
                - chrono::Duration::days(days as i64))
            .to_rfc3339());
        }
        args
    }
    fn extract_page(&self, data: &Value, _: Option<&str>) -> PageFetch {
        PageFetch {
            items: first_array(
                data,
                &[
                    "/data/items",
                    "/items",
                    "/data/data/items",
                    "/data/events",
                    "/events",
                ],
            ),
            next: next_page_token(data),
        }
    }
    fn dedup_key(&self, item: &Value) -> Option<String> {
        let id = pick_str(item, &["id", "data.id", "iCalUID", "data.iCalUID"])?;
        Some(match self.sort_cursor(item) {
            Some(updated) => format!("{id}@{updated}"),
            None => id,
        })
    }
    fn sort_cursor(&self, item: &Value) -> Option<String> {
        pick_str(item, &["updated", "data.updated"])
    }
    async fn document(
        &self,
        _: &SyncScope,
        connection_id: &str,
        item: SyncItem,
        _: &dyn ActionExecutor,
        _: &mut SyncState,
    ) -> anyhow::Result<SkillDocument> {
        let id = pick_str(&item.raw, &["id", "data.id", "iCalUID", "data.iCalUID"])
            .unwrap_or_else(|| item.dedup_key.clone());
        let title = pick_str(
            &item.raw,
            &["summary", "data.summary", "title", "data.title"],
        )
        .unwrap_or_else(|| format!("Calendar event {id}"));
        let content = serde_json::to_string_pretty(&item.raw)?;
        Ok(document(
            "googlecalendar",
            connection_id,
            &id,
            title,
            content,
            item.raw,
        ))
    }
}
