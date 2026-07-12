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

const ACTION_FETCH: &str = "NOTION_FETCH_DATA";
const ACTION_MARKDOWN: &str = "NOTION_GET_PAGE_MARKDOWN";

pub struct NotionSyncPipeline {
    client: ComposioClient,
    connection_id: String,
    max_pages: usize,
    page_size: usize,
}

impl NotionSyncPipeline {
    pub fn new(client: ComposioClient, connection_id: impl Into<String>) -> Self {
        Self {
            client,
            connection_id: connection_id.into(),
            max_pages: 20,
            page_size: 25,
        }
    }
}

#[async_trait]
impl SyncPipeline for NotionSyncPipeline {
    fn id(&self) -> &str {
        "composio:notion"
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
impl IncrementalSource for NotionSyncPipeline {
    fn toolkit(&self) -> &'static str {
        "notion"
    }
    fn action(&self) -> &'static str {
        ACTION_FETCH
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
        let mut args = serde_json::json!({"page_size": self.page_size, "filter": {"value": "page", "property": "object"}, "sort": {"direction": "descending", "timestamp": "last_edited_time"}});
        if let Some(page) = page {
            args["start_cursor"] = serde_json::json!(page);
        }
        args
    }
    fn extract_page(&self, data: &Value, _: Option<&str>) -> PageFetch {
        PageFetch {
            items: first_array(
                data,
                &[
                    "/data/results",
                    "/results",
                    "/data/data/results",
                    "/data/items",
                    "/items",
                ],
            ),
            next: [
                "/data/next_cursor",
                "/next_cursor",
                "/data/data/next_cursor",
            ]
            .iter()
            .find_map(|path| data.pointer(path).and_then(Value::as_str))
            .map(str::to_owned),
        }
    }
    fn dedup_key(&self, item: &Value) -> Option<String> {
        let id = pick_str(item, &["id", "data.id", "pageId", "data.pageId"])?;
        Some(match self.sort_cursor(item) {
            Some(edited) => format!("{id}@{edited}"),
            None => id,
        })
    }
    fn sort_cursor(&self, item: &Value) -> Option<String> {
        pick_str(
            item,
            &[
                "last_edited_time",
                "data.last_edited_time",
                "lastEditedTime",
                "data.lastEditedTime",
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
        let id = pick_str(&item.raw, &["id", "data.id", "pageId", "data.pageId"])
            .unwrap_or_else(|| item.dedup_key.clone());
        let title = notion_title(&item.raw).unwrap_or_else(|| format!("Notion page {id}"));
        let response = checked_execute(
            executor,
            ACTION_MARKDOWN,
            serde_json::json!({"page_id": id}),
            connection_id,
            state,
        )
        .await?;
        let content = [
            "/markdown",
            "/data/markdown",
            "/data/response_data/markdown",
            "/response_data/markdown",
            "/data/content",
            "/content",
            "/text",
            "/data/text",
        ]
        .iter()
        .find_map(|path| response.data.pointer(path).and_then(Value::as_str))
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .unwrap_or(serde_json::to_string_pretty(&item.raw)?);
        Ok(document(
            "notion",
            connection_id,
            &id,
            title,
            content,
            item.raw,
        ))
    }
}

fn notion_title(page: &Value) -> Option<String> {
    let properties = page
        .get("properties")
        .or_else(|| page.pointer("/data/properties"));
    properties
        .and_then(Value::as_object)
        .and_then(|props| {
            props.values().find_map(|property| {
                (property.get("type").and_then(Value::as_str) == Some("title"))
                    .then(|| {
                        property
                            .get("title")
                            .and_then(Value::as_array)
                            .map(|parts| {
                                parts
                                    .iter()
                                    .filter_map(|part| {
                                        part.get("plain_text").and_then(Value::as_str)
                                    })
                                    .collect::<Vec<_>>()
                                    .join("")
                            })
                    })
                    .flatten()
                    .filter(|title| !title.is_empty())
            })
        })
        .or_else(|| pick_str(page, &["title", "data.title", "name", "data.name"]))
}
