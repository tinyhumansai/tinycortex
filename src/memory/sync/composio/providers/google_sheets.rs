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

const ACTION_SEARCH: &str = "GOOGLESHEETS_SEARCH_SPREADSHEETS";
const ACTION_INFO: &str = "GOOGLESHEETS_GET_SPREADSHEET_INFO";

/// Incremental Google Sheets synchronization through Composio.
///
/// Two-step, document-shaped (like `NotionSyncPipeline`): `GOOGLESHEETS_SEARCH_SPREADSHEETS`
/// enumerates accessible spreadsheets, then `GOOGLESHEETS_GET_SPREADSHEET_INFO` fetches the
/// spreadsheet metadata for each item inside [`IncrementalSource::document`].
pub struct GoogleSheetsSyncPipeline {
    client: ComposioClient,
    connection_id: String,
    max_pages: usize,
    page_size: usize,
}

impl GoogleSheetsSyncPipeline {
    pub fn new(client: ComposioClient, connection_id: impl Into<String>) -> Self {
        Self {
            client,
            connection_id: connection_id.into(),
            // NOTE: SEARCH_SPREADSHEETS' page-token arg name is not pinned by the
            // curated catalog, so we do a bounded, single-page-per-tick fetch
            // (no page token emitted) rather than guessing a pagination scheme.
            max_pages: 5,
            page_size: 25,
        }
    }
}

#[async_trait]
impl SyncPipeline for GoogleSheetsSyncPipeline {
    fn id(&self) -> &str {
        "composio:googlesheets"
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
impl IncrementalSource for GoogleSheetsSyncPipeline {
    fn toolkit(&self) -> &'static str {
        "googlesheets"
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
        _page: Option<&str>,
    ) -> Value {
        // NOTE: an empty/broad `query` enumerates every accessible spreadsheet;
        // `max_results` bounds the batch. Both mirror the underlying Drive
        // search parameters. No page token is emitted (see `max_pages`).
        serde_json::json!({"query": "", "max_results": self.page_size})
    }
    fn extract_page(&self, data: &Value, _: Option<&str>) -> PageFetch {
        PageFetch {
            items: first_array(
                data,
                &[
                    "/data/spreadsheets",
                    "/spreadsheets",
                    "/data/files",
                    "/files",
                    "/data/results",
                    "/results",
                    "/data/items",
                    "/items",
                ],
            ),
            // Bounded fetch: no page token consumed (see `max_pages`). The
            // pointers are read defensively should Composio surface one.
            next: [
                "/data/nextPageToken",
                "/nextPageToken",
                "/data/next_page_token",
                "/next_page_token",
            ]
            .iter()
            .find_map(|path| data.pointer(path).and_then(Value::as_str))
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .map(str::to_owned),
        }
    }
    fn dedup_key(&self, item: &Value) -> Option<String> {
        let id = pick_str(
            item,
            &["id", "data.id", "spreadsheetId", "data.spreadsheetId"],
        )?;
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
        executor: &dyn ActionExecutor,
        state: &mut SyncState,
    ) -> anyhow::Result<SkillDocument> {
        let id = pick_str(
            &item.raw,
            &["id", "data.id", "spreadsheetId", "data.spreadsheetId"],
        )
        .unwrap_or_else(|| item.dedup_key.clone());
        let title = pick_str(
            &item.raw,
            &[
                "title",
                "data.title",
                "properties.title",
                "data.properties.title",
                "name",
            ],
        )
        .unwrap_or_else(|| format!("Google Sheet {id}"));
        // NOTE: GET_SPREADSHEET_INFO identifies the spreadsheet by a
        // "spreadsheet_id" argument (Google's canonical parameter name).
        let response = checked_execute(
            executor,
            ACTION_INFO,
            serde_json::json!({"spreadsheet_id": id}),
            connection_id,
            state,
        )
        .await?;
        // `response.data` is the already-unwrapped payload; the pointers catch
        // any additional Composio wrapping, else we serialize the payload root.
        let info = ["/data", "/data/data"]
            .iter()
            .find_map(|path| response.data.pointer(path))
            .unwrap_or(&response.data);
        let content = serde_json::to_string_pretty(info)?;
        Ok(document(
            "googlesheets",
            connection_id,
            &id,
            title,
            content,
            item.raw,
        ))
    }
}
