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

const ACTION_USERS: &str = "LINEAR_LIST_LINEAR_USERS";
const ACTION_ISSUES: &str = "LINEAR_LIST_LINEAR_ISSUES";

pub struct LinearSyncPipeline {
    client: ComposioClient,
    connection_id: String,
    max_pages: usize,
    page_size: usize,
}

impl LinearSyncPipeline {
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
impl SyncPipeline for LinearSyncPipeline {
    fn id(&self) -> &str {
        "composio:linear"
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
impl IncrementalSource for LinearSyncPipeline {
    fn toolkit(&self) -> &'static str {
        "linear"
    }
    fn action(&self) -> &'static str {
        ACTION_ISSUES
    }
    fn max_pages(&self) -> usize {
        self.max_pages
    }
    async fn scopes(
        &self,
        executor: &dyn ActionExecutor,
        connection_id: &str,
        state: &mut SyncState,
    ) -> anyhow::Result<Vec<SyncScope>> {
        let response = checked_execute(
            executor,
            ACTION_USERS,
            serde_json::json!({"isMe": true}),
            connection_id,
            state,
        )
        .await?;
        let users = first_array(
            &response.data,
            &[
                "/data/nodes",
                "/nodes",
                "/data/data/nodes",
                "/data/users/nodes",
            ],
        );
        let viewer = users.first().unwrap_or(&response.data);
        let id = pick_str(viewer, &["id", "data.id"])
            .ok_or_else(|| anyhow::anyhow!("{ACTION_USERS} returned no viewer id"))?;
        Ok(vec![SyncScope::named(id, "assignee:me")])
    }
    fn arguments(
        &self,
        scope: &SyncScope,
        _: &MemoryConfig,
        _: &SyncState,
        page: Option<&str>,
    ) -> Value {
        let mut args = serde_json::json!({"assigneeId": scope.id, "first": self.page_size, "orderBy": "updatedAt"});
        if let Some(page) = page {
            args["after"] = serde_json::json!(page);
        }
        args
    }
    fn extract_page(&self, data: &Value, _: Option<&str>) -> PageFetch {
        let items = first_array(
            data,
            &[
                "/data/nodes",
                "/nodes",
                "/data/data/nodes",
                "/data/issues/nodes",
                "/data/results",
                "/results",
                "/data/items",
                "/items",
            ],
        );
        let page_info = [
            "/data/pageInfo",
            "/pageInfo",
            "/data/data/pageInfo",
            "/data/issues/pageInfo",
        ]
        .iter()
        .find_map(|path| data.pointer(path));
        let next = page_info
            .filter(|info| {
                info.get("hasNextPage")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            })
            .and_then(|info| info.get("endCursor").and_then(Value::as_str))
            .map(str::to_owned);
        PageFetch { items, next }
    }
    fn dedup_key(&self, item: &Value) -> Option<String> {
        let id = pick_str(item, &["id", "data.id", "identifier", "data.identifier"])?;
        Some(match self.sort_cursor(item) {
            Some(updated) => format!("{id}@{updated}"),
            None => id,
        })
    }
    fn sort_cursor(&self, item: &Value) -> Option<String> {
        pick_str(
            item,
            &[
                "updatedAt",
                "data.updatedAt",
                "updated_at",
                "data.updated_at",
            ],
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
        let id = pick_str(
            &item.raw,
            &["id", "data.id", "identifier", "data.identifier"],
        )
        .unwrap_or_else(|| item.dedup_key.clone());
        let title = pick_str(
            &item.raw,
            &[
                "title",
                "data.title",
                "name",
                "data.name",
                "identifier",
                "data.identifier",
            ],
        )
        .unwrap_or_else(|| format!("Linear issue {id}"));
        let content = serde_json::to_string_pretty(&item.raw)?;
        Ok(document(
            "linear",
            connection_id,
            &id,
            title,
            content,
            item.raw,
        ))
    }
}
