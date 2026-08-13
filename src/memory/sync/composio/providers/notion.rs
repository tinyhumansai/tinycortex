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
        let body = [
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
        // `NOTION_GET_PAGE_MARKDOWN` returns page *block* content only, so a
        // database-row page's structured property values (select / status /
        // multi_select / date / …) never appear in `body`. Render them from the
        // fetched row and prepend, so the agent reads the real dropdown
        // selections instead of inventing them (#5500).
        let properties = render_properties(&item.raw);
        let content = if properties.is_empty() {
            body
        } else {
            format!("{properties}\n\n{body}")
        };
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

/// Render a Notion row's structured database properties into readable
/// `Name: value` lines under a `Properties:` header.
///
/// The sync body comes from `NOTION_GET_PAGE_MARKDOWN`, which returns page
/// *block* content only — a database row's property values
/// (`select`/`status`/`multi_select`/`date`/`people`/`relation`/scalars) are
/// **not** in that markdown. Without this, a tracker page reaches the agent with
/// no dropdown text and the model invents the selections (#5500). Values are
/// read from the already-fetched row (`item.raw`, the same object
/// [`notion_title`] reads), so no extra Composio call is needed. The `title`
/// property is skipped here (it is already the document title); empty / null
/// values are skipped. Returns an empty string when nothing renders.
fn render_properties(page: &Value) -> String {
    let Some(properties) = page
        .get("properties")
        .or_else(|| page.pointer("/data/properties"))
        .and_then(Value::as_object)
    else {
        return String::new();
    };
    let mut lines: Vec<String> = properties
        .iter()
        .filter_map(|(name, property)| {
            let kind = property.get("type").and_then(Value::as_str)?;
            let value = match kind {
                // Already surfaced as the document title.
                "title" => return None,
                "rich_text" => plain_text(property.get("rich_text")),
                "select" | "status" => property
                    .get(kind)
                    .and_then(|inner| inner.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                "multi_select" => named_list(property.get("multi_select")),
                "people" => named_list(property.get("people")),
                "relation" => property
                    .get("relation")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(|item| item.get("id").and_then(Value::as_str))
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default(),
                "date" => property
                    .get("date")
                    .and_then(Value::as_object)
                    .map(|date| {
                        let start = date
                            .get("start")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        match date.get("end").and_then(Value::as_str) {
                            Some(end) if !end.is_empty() => format!("{start} → {end}"),
                            _ => start.to_string(),
                        }
                    })
                    .unwrap_or_default(),
                "checkbox" => match property.get("checkbox").and_then(Value::as_bool) {
                    Some(true) => "Yes".to_string(),
                    Some(false) => "No".to_string(),
                    None => String::new(),
                },
                "number" => property
                    .get("number")
                    .filter(|value| value.is_number())
                    .map(Value::to_string)
                    .unwrap_or_default(),
                "url" | "email" | "phone_number" => property
                    .get(kind)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                _ => String::new(),
            };
            let value = value.trim();
            (!value.is_empty()).then(|| format!("{name}: {value}"))
        })
        .collect();
    if lines.is_empty() {
        return String::new();
    }
    // Stable ordering so the synced document is deterministic across runs
    // (serde_json preserves object insertion order only with the `preserve_order`
    // feature, which is not enabled here).
    lines.sort();
    format!("Properties:\n{}", lines.join("\n"))
}

/// Join the `plain_text` runs of a Notion rich-text array into a single string.
fn plain_text(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_array)
        .map(|parts| {
            parts
                .iter()
                .filter_map(|part| part.get("plain_text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default()
}

/// Comma-join the `name` field of every object in a Notion array property
/// (`multi_select` options, `people` entries).
fn named_list(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("name").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
}
