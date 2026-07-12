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

const ACTION_USER: &str = "GITHUB_GET_THE_AUTHENTICATED_USER";
const ACTION_SEARCH: &str = "GITHUB_SEARCH_ISSUES_AND_PULL_REQUESTS";

pub struct GitHubSyncPipeline {
    client: ComposioClient,
    connection_id: String,
    max_pages: usize,
    page_size: usize,
}

impl GitHubSyncPipeline {
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
impl SyncPipeline for GitHubSyncPipeline {
    fn id(&self) -> &str {
        "composio:github"
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
impl IncrementalSource for GitHubSyncPipeline {
    fn toolkit(&self) -> &'static str {
        "github"
    }
    fn action(&self) -> &'static str {
        ACTION_SEARCH
    }
    fn max_pages(&self) -> usize {
        self.max_pages
    }
    fn server_side_depth(&self) -> bool {
        true
    }
    async fn scopes(
        &self,
        executor: &dyn ActionExecutor,
        connection_id: &str,
        state: &mut SyncState,
    ) -> anyhow::Result<Vec<SyncScope>> {
        let response = checked_execute(
            executor,
            ACTION_USER,
            serde_json::json!({}),
            connection_id,
            state,
        )
        .await?;
        let login = pick_str(&response.data, &["login", "data.login"])
            .ok_or_else(|| anyhow::anyhow!("{ACTION_USER} returned no login"))?;
        Ok(vec![SyncScope::named(
            login.clone(),
            format!("involves:{login}"),
        )])
    }
    fn arguments(
        &self,
        scope: &SyncScope,
        config: &MemoryConfig,
        state: &SyncState,
        page: Option<&str>,
    ) -> Value {
        let mut query = format!("involves:{}", scope.id);
        if let Some(cursor) = state.cursor.as_deref() {
            query.push_str(&format!(" updated:>{cursor}"));
        } else if let Some(days) = config.sync.budget.sync_depth_days {
            let floor = chrono::Utc::now() - chrono::Duration::days(days as i64);
            query.push_str(&format!(" updated:>{}", floor.format("%Y-%m-%dT%H:%M:%SZ")));
        }
        serde_json::json!({ "q": query, "sort": "updated", "order": "desc", "per_page": self.page_size, "page": page.and_then(|value| value.parse::<u32>().ok()).unwrap_or(1) })
    }
    fn extract_page(&self, data: &Value, page: Option<&str>) -> PageFetch {
        let items = first_array(
            data,
            &[
                "/data/items",
                "/items",
                "/data/data/items",
                "/data/results",
                "/results",
            ],
        );
        let page_number = page
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(1);
        let next = (items.len() == self.page_size).then(|| (page_number + 1).to_string());
        PageFetch { items, next }
    }
    fn dedup_key(&self, item: &Value) -> Option<String> {
        let id = issue_id(item)?;
        Some(match self.sort_cursor(item) {
            Some(updated) => format!("{id}@{updated}"),
            None => id,
        })
    }
    fn sort_cursor(&self, item: &Value) -> Option<String> {
        pick_str(
            item,
            &[
                "updated_at",
                "data.updated_at",
                "updatedAt",
                "data.updatedAt",
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
        let id = issue_id(&item.raw).unwrap_or_else(|| item.dedup_key.clone());
        let title = pick_str(&item.raw, &["title", "data.title"])
            .unwrap_or_else(|| format!("GitHub issue {id}"));
        let content = serde_json::to_string_pretty(&item.raw)?;
        Ok(document(
            "github",
            connection_id,
            &id,
            title,
            content,
            item.raw,
        ))
    }
}

fn issue_id(item: &Value) -> Option<String> {
    pick_str(item, &["id", "data.id"]).or_else(|| {
        let url = pick_str(item, &["html_url", "data.html_url", "url", "data.url"])?;
        let parts: Vec<_> = url.trim_end_matches('/').split('/').collect();
        (parts.len() >= 7).then(|| {
            format!(
                "{}/{}#{}",
                parts[parts.len() - 4],
                parts[parts.len() - 3],
                parts[parts.len() - 1]
            )
        })
    })
}
