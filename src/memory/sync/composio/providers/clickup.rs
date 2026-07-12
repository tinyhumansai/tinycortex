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

const ACTION_USER: &str = "CLICKUP_GET_AUTHORIZED_USER";
const ACTION_WORKSPACES: &str = "CLICKUP_GET_AUTHORIZED_TEAMS_WORKSPACES";
const ACTION_TASKS: &str = "CLICKUP_GET_FILTERED_TEAM_TASKS";

pub struct ClickUpSyncPipeline {
    client: ComposioClient,
    connection_id: String,
    max_pages: usize,
    page_size: usize,
}

impl ClickUpSyncPipeline {
    pub fn new(client: ComposioClient, connection_id: impl Into<String>) -> Self {
        Self {
            client,
            connection_id: connection_id.into(),
            max_pages: 20,
            page_size: 50,
        }
    }
}

#[async_trait]
impl SyncPipeline for ClickUpSyncPipeline {
    fn id(&self) -> &str {
        "composio:clickup"
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
impl IncrementalSource for ClickUpSyncPipeline {
    fn toolkit(&self) -> &'static str {
        "clickup"
    }
    fn action(&self) -> &'static str {
        ACTION_TASKS
    }
    fn max_pages(&self) -> usize {
        self.max_pages
    }
    fn depth_floor(&self, config: &MemoryConfig, state: &SyncState) -> Option<String> {
        if state.cursor.is_some() {
            return None;
        }
        config.sync.budget.sync_depth_days.map(|days| {
            (chrono::Utc::now() - chrono::Duration::days(days as i64))
                .timestamp_millis()
                .to_string()
        })
    }
    async fn scopes(
        &self,
        executor: &dyn ActionExecutor,
        connection_id: &str,
        state: &mut SyncState,
    ) -> anyhow::Result<Vec<SyncScope>> {
        let user_response = checked_execute(
            executor,
            ACTION_USER,
            serde_json::json!({}),
            connection_id,
            state,
        )
        .await?;
        let user_id = ["/user/id", "/data/user/id", "/id", "/data/id"]
            .iter()
            .find_map(|path| user_response.data.pointer(path))
            .and_then(value_string)
            .ok_or_else(|| anyhow::anyhow!("{ACTION_USER} returned no user id"))?;
        if state.budget_exhausted() {
            return Ok(Vec::new());
        }
        let workspace_response = checked_execute(
            executor,
            ACTION_WORKSPACES,
            serde_json::json!({}),
            connection_id,
            state,
        )
        .await?;
        let workspaces = first_array(
            &workspace_response.data,
            &["/teams", "/data/teams", "/workspaces", "/data/workspaces"],
        );
        Ok(workspaces
            .into_iter()
            .filter_map(|workspace| pick_str(&workspace, &["id", "team_id", "workspace_id"]))
            .map(|id| {
                SyncScope::named(id.clone(), format!("workspace:{id}"))
                    .with_metadata(serde_json::json!({"user_id": user_id}))
            })
            .collect())
    }
    fn arguments(
        &self,
        scope: &SyncScope,
        _: &MemoryConfig,
        _: &SyncState,
        page: Option<&str>,
    ) -> Value {
        serde_json::json!({"team_id": scope.id, "assignees": [scope.metadata.get("user_id").and_then(Value::as_str).unwrap_or_default()], "order_by": "updated", "reverse": true, "page": page.and_then(|value| value.parse::<u32>().ok()).unwrap_or(0), "page_size": self.page_size, "subtasks": true})
    }
    fn extract_page(&self, data: &Value, page: Option<&str>) -> PageFetch {
        let items = first_array(
            data,
            &[
                "/data/tasks",
                "/tasks",
                "/data/data/tasks",
                "/data/results",
                "/results",
                "/data/items",
                "/items",
            ],
        );
        let page_number = page
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(0);
        let next = (items.len() == self.page_size).then(|| (page_number + 1).to_string());
        PageFetch { items, next }
    }
    fn dedup_key(&self, item: &Value) -> Option<String> {
        let id = pick_str(item, &["id", "data.id", "task_id", "data.task_id"])?;
        Some(match self.sort_cursor(item) {
            Some(updated) => format!("{id}@{updated}"),
            None => id,
        })
    }
    fn sort_cursor(&self, item: &Value) -> Option<String> {
        pick_str(
            item,
            &[
                "date_updated",
                "data.date_updated",
                "updated_at",
                "data.updated_at",
                "dateUpdated",
                "data.dateUpdated",
            ],
        )
    }
    async fn document(
        &self,
        scope: &SyncScope,
        connection_id: &str,
        item: SyncItem,
        _: &dyn ActionExecutor,
        _: &mut SyncState,
    ) -> anyhow::Result<SkillDocument> {
        let id = pick_str(&item.raw, &["id", "data.id", "task_id", "data.task_id"])
            .unwrap_or_else(|| item.dedup_key.clone());
        let title = pick_str(&item.raw, &["name", "data.name", "title", "data.title"])
            .unwrap_or_else(|| format!("ClickUp task {id}"));
        let content = serde_json::to_string_pretty(&item.raw)?;
        let mut result = document("clickup", connection_id, &id, title, content, item.raw);
        result.metadata["workspace_id"] = Value::String(scope.id.clone());
        Ok(result)
    }
}

fn value_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
    .map(|value| value.trim().to_owned())
    .filter(|value| !value.is_empty())
}
