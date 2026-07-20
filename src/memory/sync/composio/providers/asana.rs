//! Incremental Asana synchronization through Composio.
//!
//! Asana is a task-shaped source: we enumerate the connected user's projects
//! (across every accessible workspace) and page each project's tasks. The
//! provider mirrors the GitHub pipeline's *server-side depth* strategy — Asana
//! filters by `modified_since`, so every fetched task is strictly newer than the
//! persisted cursor and the orchestrator's cursor-boundary check never truncates
//! an unordered page. Combined with a stable `asana:<gid>` document id, re-syncs
//! are idempotent (see `openhuman#4953`: the upsert key must be the stable id,
//! never a per-run id).

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

const ACTION_WORKSPACES: &str = "ASANA_GET_MULTIPLE_WORKSPACES";
const ACTION_PROJECTS: &str = "ASANA_GET_MULTIPLE_PROJECTS";
const ACTION_TASKS: &str = "ASANA_GET_MULTIPLE_TASKS";

/// Asana returns only `gid`/`name`/`resource_type` unless `opt_fields` is set;
/// `modified_at` is required for the incremental cursor.
const TASK_OPT_FIELDS: &str =
    "name,modified_at,created_at,completed,completed_at,notes,permalink_url,assignee.name,due_on";

pub struct AsanaSyncPipeline {
    client: ComposioClient,
    connection_id: String,
    max_pages: usize,
    page_size: usize,
}

impl AsanaSyncPipeline {
    pub fn new(client: ComposioClient, connection_id: impl Into<String>) -> Self {
        Self {
            client,
            connection_id: connection_id.into(),
            max_pages: 20,
            page_size: 50,
        }
    }

    /// Time floor for the incremental fetch: the persisted cursor once one
    /// exists, otherwise the configured backfill depth. Formatted as the RFC
    /// 3339 timestamp Asana's `modified_since` expects.
    fn modified_since(&self, config: &MemoryConfig, state: &SyncState) -> Option<String> {
        if let Some(cursor) = state.cursor.as_deref() {
            return Some(cursor.to_owned());
        }
        config.sync.budget.sync_depth_days.map(|days| {
            (chrono::Utc::now() - chrono::Duration::days(days as i64))
                .format("%Y-%m-%dT%H:%M:%SZ")
                .to_string()
        })
    }
}

#[async_trait]
impl SyncPipeline for AsanaSyncPipeline {
    fn id(&self) -> &str {
        "composio:asana"
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
impl IncrementalSource for AsanaSyncPipeline {
    fn toolkit(&self) -> &'static str {
        "asana"
    }
    fn action(&self) -> &'static str {
        ACTION_TASKS
    }
    fn max_pages(&self) -> usize {
        self.max_pages
    }
    /// Asana filters by `modified_since` server-side, so the engine must not
    /// also apply a client-side depth floor.
    fn server_side_depth(&self) -> bool {
        true
    }
    /// A single unreachable project must not abort the whole sync.
    fn tolerate_scope_errors(&self) -> bool {
        true
    }
    async fn scopes(
        &self,
        executor: &dyn ActionExecutor,
        connection_id: &str,
        state: &mut SyncState,
    ) -> anyhow::Result<Vec<SyncScope>> {
        let workspaces_response = checked_execute(
            executor,
            ACTION_WORKSPACES,
            serde_json::json!({}),
            connection_id,
            state,
        )
        .await?;
        let workspaces = extract_records(&workspaces_response.data);
        let mut scopes = Vec::new();
        for workspace in &workspaces {
            let Some(workspace_id) = record_id(workspace) else {
                continue;
            };
            if state.budget_exhausted() {
                break;
            }
            let projects_response = checked_execute(
                executor,
                ACTION_PROJECTS,
                serde_json::json!({
                    "workspace": workspace_id,
                    "archived": false,
                    "limit": 100,
                    "opt_fields": "name",
                }),
                connection_id,
                state,
            )
            .await?;
            let projects = extract_records(&projects_response.data);
            for project in &projects {
                let Some(project_id) = record_id(project) else {
                    continue;
                };
                let name = pick_str(project, &["name", "data.name"])
                    .unwrap_or_else(|| format!("project {project_id}"));
                scopes.push(
                    SyncScope::named(project_id.clone(), format!("project:{project_id}"))
                        .with_metadata(serde_json::json!({
                            "workspace_id": workspace_id,
                            "project_name": name,
                        })),
                );
            }
        }
        tracing::debug!(
            toolkit = "asana",
            connection_id,
            workspaces = workspaces.len(),
            projects = scopes.len(),
            "[sync:asana] resolved project scopes"
        );
        Ok(scopes)
    }
    fn arguments(
        &self,
        scope: &SyncScope,
        config: &MemoryConfig,
        state: &SyncState,
        page: Option<&str>,
    ) -> Value {
        let mut args = serde_json::json!({
            "project": scope.id,
            "limit": self.page_size,
            "opt_fields": TASK_OPT_FIELDS,
        });
        if let Some(since) = self.modified_since(config, state) {
            args["modified_since"] = Value::String(since);
        }
        if let Some(offset) = page {
            args["offset"] = Value::String(offset.to_owned());
        }
        args
    }
    fn extract_page(&self, data: &Value, _: Option<&str>) -> PageFetch {
        PageFetch {
            items: extract_records(data),
            next: [
                "/data/next_page/offset",
                "/next_page/offset",
                "/data/data/next_page/offset",
                "/response_data/next_page/offset",
            ]
            .iter()
            .find_map(|path| data.pointer(path).and_then(Value::as_str))
            .map(str::trim)
            .filter(|offset| !offset.is_empty())
            .map(str::to_owned),
        }
    }
    fn dedup_key(&self, item: &Value) -> Option<String> {
        let id = task_id(item)?;
        Some(match self.sort_cursor(item) {
            Some(modified) => format!("{id}@{modified}"),
            None => id,
        })
    }
    fn sort_cursor(&self, item: &Value) -> Option<String> {
        pick_str(
            item,
            &[
                "modified_at",
                "data.modified_at",
                "modified_on",
                "data.modified_on",
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
        let id = task_id(&item.raw).unwrap_or_else(|| item.dedup_key.clone());
        let title = pick_str(&item.raw, &["name", "data.name", "title", "data.title"])
            .unwrap_or_else(|| format!("Asana task {id}"));
        let content = serde_json::to_string_pretty(&item.raw)?;
        // `document()` sets `document_id = asana:<gid>` and `taint = external_sync`.
        let mut result = document("asana", connection_id, &id, title, content, item.raw);
        // Stable collection scope for dedupe/path grouping — the project, never a
        // per-run identifier.
        result.metadata["path_scope"] = Value::String(format!("asana/project/{}", scope.id));
        result.metadata["project_id"] = Value::String(scope.id.clone());
        if let Some(workspace_id) = scope.metadata.get("workspace_id").and_then(Value::as_str) {
            result.metadata["workspace_id"] = Value::String(workspace_id.to_owned());
        }
        Ok(result)
    }
}

/// Asana wraps its collection payloads in `data`, and Composio wraps the
/// tool response again — so the task/project/workspace array may sit under
/// `/data/data`, `/data`, or the bare root depending on envelope nesting.
fn extract_records(data: &Value) -> Vec<Value> {
    first_array(
        data,
        &[
            "/data/data",
            "/data",
            "/response_data/data",
            "/data/response_data/data",
            "/data/items",
            "/items",
        ],
    )
}

fn record_id(record: &Value) -> Option<String> {
    pick_str(record, &["gid", "id", "data.gid", "data.id"])
}

fn task_id(item: &Value) -> Option<String> {
    pick_str(item, &["gid", "id", "data.gid", "data.id", "task_gid"])
}

#[cfg(test)]
#[path = "asana_tests.rs"]
mod tests;
