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

const ACTION_SEARCH: &str = "GOOGLEDOCS_SEARCH_DOCUMENTS";
const ACTION_PLAINTEXT: &str = "GOOGLEDOCS_GET_DOCUMENT_PLAINTEXT";

/// Incremental Google Docs synchronization through Composio.
///
/// Two-step, document-shaped (like `NotionSyncPipeline`): `GOOGLEDOCS_SEARCH_DOCUMENTS`
/// enumerates accessible documents, then `GOOGLEDOCS_GET_DOCUMENT_PLAINTEXT` fetches the
/// body for each item inside [`IncrementalSource::document`].
pub struct GoogleDocsSyncPipeline {
    client: ComposioClient,
    connection_id: String,
    max_pages: usize,
    page_size: usize,
}

impl GoogleDocsSyncPipeline {
    pub fn new(client: ComposioClient, connection_id: impl Into<String>) -> Self {
        Self {
            client,
            connection_id: connection_id.into(),
            // NOTE: SEARCH_DOCUMENTS' page-token arg name is not pinned by the
            // curated catalog, so we do a bounded, single-page-per-tick fetch
            // (no page token emitted) rather than guessing a pagination scheme.
            max_pages: 5,
            page_size: 25,
        }
    }
}

#[async_trait]
impl SyncPipeline for GoogleDocsSyncPipeline {
    fn id(&self) -> &str {
        "composio:googledocs"
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
impl IncrementalSource for GoogleDocsSyncPipeline {
    fn toolkit(&self) -> &'static str {
        "googledocs"
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
        // NOTE: an empty/broad `query` enumerates every accessible document;
        // `max_results` bounds the batch. Both mirror the underlying Drive
        // search parameters. No page token is emitted (see `max_pages`).
        serde_json::json!({"query": "", "max_results": self.page_size})
    }
    fn extract_page(&self, data: &Value, _: Option<&str>) -> PageFetch {
        PageFetch {
            items: first_array(
                data,
                &[
                    "/data/documents",
                    "/documents",
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
        let id = pick_str(item, &["id", "data.id", "documentId", "data.documentId"])?;
        Some(match self.sort_cursor(item) {
            Some(modified) => format!("{id}@{modified}"),
            None => id,
        })
    }
    fn sort_cursor(&self, item: &Value) -> Option<String> {
        pick_str(
            item,
            &[
                "modifiedTime",
                "data.modifiedTime",
                "modified_time",
                "updatedTime",
            ],
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
            &["id", "data.id", "documentId", "data.documentId"],
        )
        .unwrap_or_else(|| item.dedup_key.clone());
        let title = pick_str(&item.raw, &["title", "data.title", "name", "data.name"])
            .unwrap_or_else(|| format!("Google Doc {id}"));
        // NOTE: GET_DOCUMENT_PLAINTEXT identifies the doc by an id argument;
        // Composio commonly keys this as "id" (or "document_id"). We send "id".
        let response = checked_execute(
            executor,
            ACTION_PLAINTEXT,
            serde_json::json!({"id": id}),
            connection_id,
            state,
        )
        .await?;
        let content = [
            "/data/text",
            "/text",
            "/data/plaintext",
            "/plaintext",
            "/data/content",
            "/content",
            "/data/response_data/text",
        ]
        .iter()
        .find_map(|path| response.data.pointer(path).and_then(Value::as_str))
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .unwrap_or(serde_json::to_string_pretty(&item.raw)?);
        Ok(document(
            "googledocs",
            connection_id,
            &id,
            title,
            content,
            item.raw,
        ))
    }
}
