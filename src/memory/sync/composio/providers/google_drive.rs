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

// Composio deprecated `GOOGLEDRIVE_LIST_FILES` (2026-03-28) in favour of
// `GOOGLEDRIVE_FIND_FILE`, which is the current `files.list`-backed listing
// action (same paging/ordering/`q` filter surface).
const ACTION_FIND_FILE: &str = "GOOGLEDRIVE_FIND_FILE";

/// Incremental Google Drive synchronization through Composio.
///
/// File-shaped: each Drive file is a record with a stable id and a
/// `modifiedTime`. This indexes file *metadata* only — it never downloads
/// binary bodies (which may be arbitrarily large and are not memory-shaped);
/// the document content is the file's structured metadata.
pub struct GoogleDriveSyncPipeline {
    client: ComposioClient,
    connection_id: String,
    max_pages: usize,
    page_size: usize,
}

impl GoogleDriveSyncPipeline {
    pub fn new(client: ComposioClient, connection_id: impl Into<String>) -> Self {
        Self {
            client,
            connection_id: connection_id.into(),
            max_pages: 10,
            page_size: 50,
        }
    }

    pub fn with_limits(mut self, max_pages: usize, page_size: usize) -> Self {
        self.max_pages = max_pages.max(1);
        // Google Drive caps `pageSize` at 1000.
        self.page_size = page_size.clamp(1, 1000);
        self
    }
}

#[async_trait]
impl SyncPipeline for GoogleDriveSyncPipeline {
    fn id(&self) -> &str {
        "composio:googledrive"
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
impl IncrementalSource for GoogleDriveSyncPipeline {
    fn toolkit(&self) -> &'static str {
        "googledrive"
    }
    fn action(&self) -> &'static str {
        ACTION_FIND_FILE
    }
    fn max_pages(&self) -> usize {
        self.max_pages
    }
    // NB: `stop_on_empty_pending` stays at its default (false) — see the note on
    // the Calendar pipeline. The cursor only advances on a complete sync, so a
    // capped run must not stop early on an all-deduplicated first page or it
    // would permanently skip the unsynced tail.
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
        let mut args = serde_json::json!({
            "page_size": self.page_size,
            "order_by": "modifiedTime desc",
            // Guarantee the fields the cursor/title/dedup depend on come back,
            // regardless of the action's default projection.
            "fields": "files(id,name,mimeType,modifiedTime),nextPageToken",
        });
        if let Some(page) = page {
            args["page_token"] = serde_json::json!(page);
        }
        // Depth window via a Drive `q` clause on modification time. Prefer the
        // last-synced cursor, else the configured horizon. The cursor is
        // validated as an RFC3339 timestamp before being interpolated into the
        // query so a malformed persisted value can never inject into the `q`
        // clause — on a bad value we simply omit the depth filter (full scan).
        let floor = state
            .cursor
            .as_deref()
            .filter(|cursor| chrono::DateTime::parse_from_rfc3339(cursor).is_ok())
            .map(str::to_owned)
            .or_else(|| {
                config.sync.budget.sync_depth_days.map(|days| {
                    (chrono::Utc::now() - chrono::Duration::days(days as i64)).to_rfc3339()
                })
            });
        if let Some(floor) = floor {
            args["query"] = serde_json::json!(format!("modifiedTime > '{floor}'"));
        }
        args
    }
    fn extract_page(&self, data: &Value, _: Option<&str>) -> PageFetch {
        PageFetch {
            items: first_array(
                data,
                &[
                    "/data/files",
                    "/files",
                    "/data/data/files",
                    "/data/items",
                    "/items",
                ],
            ),
            next: next_page_token(data),
        }
    }
    fn dedup_key(&self, item: &Value) -> Option<String> {
        let id = pick_str(item, &["id", "data.id", "fileId", "data.fileId"])?;
        Some(match self.sort_cursor(item) {
            Some(modified) => format!("{id}@{modified}"),
            None => id,
        })
    }
    fn sort_cursor(&self, item: &Value) -> Option<String> {
        pick_str(
            item,
            &["modifiedTime", "data.modifiedTime", "modified_time"],
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
        let id = pick_str(&item.raw, &["id", "data.id", "fileId", "data.fileId"])
            .unwrap_or_else(|| item.dedup_key.clone());
        let title = pick_str(&item.raw, &["name", "data.name", "title", "data.title"])
            .unwrap_or_else(|| format!("Drive file {id}"));
        let content = serde_json::to_string_pretty(&item.raw)?;
        Ok(document(
            "googledrive",
            connection_id,
            &id,
            title,
            content,
            item.raw,
        ))
    }
}
