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
        let markdown = [
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
        .map(str::to_owned);
        // `NOTION_GET_PAGE_MARKDOWN` returns page *block* content only, so a
        // database-row page's structured property values (select / status /
        // multi_select / date / …) never appear in the markdown. Render them from
        // the fetched row and prepend, so the agent reads the real dropdown
        // selections instead of inventing them (#5500).
        let content = match markdown {
            Some(body) => {
                let properties = render_properties(&item.raw);
                if properties.is_empty() {
                    body
                } else {
                    format!("{properties}\n\n{body}")
                }
            }
            // No page markdown available: fall back to the raw row JSON. It
            // already contains the `properties` object, so do NOT prepend the
            // rendered block as well — that would duplicate the property values.
            None => serde_json::to_string_pretty(&item.raw)?,
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
/// [`notion_title`] reads), so no extra Composio call is needed.
///
/// Contract for the emitted block:
/// - the `title` property is skipped (it is already the document title), and
///   empty / null values are skipped, so nothing renders as a blank `Name:`;
/// - each name and value is whitespace-collapsed to a single line, so a value
///   can't forge extra property lines (see [`collapse_ws`]);
/// - every kind is rendered through the single [`render_property_value`] table
///   (formula/rollup/unique_id/timestamps/relation/files/…), which logs anything
///   it still can't read rather than dropping it silently;
/// - lines are sorted, so the block is deterministic across runs (serde_json
///   only preserves object insertion order under the unused `preserve_order`
///   feature).
///
/// Returns an empty string when nothing renders (the caller then emits the
/// markdown body alone). Note this is prepended to real page markdown only; when
/// markdown is absent the caller falls back to the raw row JSON instead, which
/// already contains the properties, so the block is not duplicated there.
fn render_properties(page: &Value) -> String {
    let Some(properties) = page
        .get("properties")
        .or_else(|| page.pointer("/data/properties"))
        .and_then(Value::as_object)
    else {
        // No `properties` object at all — a non-database page, or an envelope
        // shaped differently than expected. Log once (the "inert against real
        // Composio" case) rather than returning silently.
        tracing::debug!("[memory_sync:notion] row has no properties object; rendered nothing");
        return String::new();
    };
    let mut lines: Vec<String> = properties
        .iter()
        .filter_map(|(name, property)| {
            let kind = property.get("type").and_then(Value::as_str)?;
            if kind == "title" {
                return None; // already surfaced as the document title
            }
            // Collapse whitespace (including newlines) in both the property name
            // and value so neither can forge extra `Name: value` lines in the
            // block — a rich_text value like "real\nStatus: FAKE" would otherwise
            // inject a second, higher-sorting property line. Keeps the one-line
            // `Name: value` contract.
            let value = collapse_ws(&render_property_value(kind, property));
            (!value.is_empty()).then(|| format!("{}: {value}", collapse_ws(name)))
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

/// The single dispatch for one Notion typed value `{ "type": kind, [kind]: … }`
/// — used for both a top-level database property and each element of a rollup
/// `array`. Every kind lives here, so no caller renders a *narrower* subset that
/// silently drops a shape (the bug class that recurs when a specialization
/// diverges from the general path). Tracker pages lean on `formula`, `rollup`,
/// `unique_id`, `relation`, and the audit timestamps/users; each is rendered
/// concretely. An unrecognised kind falls back to any bare scalar and, if that is
/// empty, is logged (`tracing::debug`) rather than vanishing — a new Notion
/// property type stays visible in telemetry.
fn render_property_value(kind: &str, property: &Value) -> String {
    match kind {
        "title" | "rich_text" => plain_text(property.get(kind)),
        "select" | "status" => property
            .get(kind)
            .and_then(|inner| inner.get("name"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        "multi_select" | "people" => named_list(property.get(kind)),
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
        "date" => format_date(property.get("date")),
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
        "created_time" | "last_edited_time" => property
            .get(kind)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        "created_by" | "last_edited_by" => property
            .get(kind)
            .and_then(|user| user.get("name"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        "unique_id" => property
            .get("unique_id")
            .and_then(Value::as_object)
            .map(|uid| {
                let number = uid.get("number").filter(|n| n.is_number());
                match (uid.get("prefix").and_then(Value::as_str), number) {
                    (Some(prefix), Some(n)) => format!("{prefix}-{n}"),
                    (_, Some(n)) => n.to_string(),
                    _ => String::new(),
                }
            })
            .unwrap_or_default(),
        // `formula`/`rollup` wrap their result in another typed value.
        "formula" | "rollup" => property
            .get(kind)
            .map(render_typed_value)
            .unwrap_or_default(),
        // `files` entries expose a `name`; reuse the object-name join.
        "files" => named_list(property.get(kind)),
        _ => {
            let rendered = scalar_value(property.get(kind));
            if rendered.trim().is_empty() {
                tracing::debug!(kind, "[memory_sync:notion] unrendered property");
            }
            rendered
        }
    }
}

/// Render a Notion "typed value" wrapper `{ "type": T, T: <inner> }` — used by
/// `formula`, `rollup`, and each element of a rollup `array`. `array` recurses;
/// a bare scalar with no `type` renders directly; every other kind delegates to
/// [`render_property_value`], so an array element of any kind renders exactly as
/// the same kind would at property level (no narrower dispatch, so no dropped
/// rollup-of-formula / rollup-of-relation).
fn render_typed_value(wrapper: &Value) -> String {
    let Some(kind) = wrapper.get("type").and_then(Value::as_str) else {
        return scalar_value(Some(wrapper));
    };
    if kind == "array" {
        return wrapper
            .get("array")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(render_typed_value)
                    .filter(|rendered| !rendered.is_empty())
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
    }
    render_property_value(kind, wrapper)
}

/// Format a Notion `date` object (`{ start, end? }`) as `start` or `start → end`.
/// Shared by the `date` property arm and formula/rollup date results.
fn format_date(value: Option<&Value>) -> String {
    value
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
        .unwrap_or_default()
}

/// Render a bare Notion scalar (`string` / `number` / `bool`) or a `{ name: … }`
/// object into text; empty for a structured shape we don't recognise. The last
/// resort for [`render_property_value`] and [`render_typed_value`], covering any
/// scalar-typed property and the flattened formula/rollup envelope.
fn scalar_value(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => if *b { "Yes" } else { "No" }.to_string(),
        Some(Value::Object(obj)) => obj
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        _ => String::new(),
    }
}

/// Collapse every run of whitespace (spaces, tabs, newlines) to a single space
/// and trim. Applied to each property name and value before it is emitted so a
/// value can't inject additional `Name: value` lines into the rendered block.
fn collapse_ws(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
