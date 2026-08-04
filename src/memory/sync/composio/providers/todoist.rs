use async_trait::async_trait;
use serde_json::Value;

use super::common::{document, first_array, pick_str};
use crate::memory::config::MemoryConfig;
use crate::memory::sync::composio::{
    run_incremental_sync, ActionExecutor, ComposioClient, IncrementalSource, PageFetch, SyncItem,
    SyncScope,
};
use crate::memory::sync::state::SyncState;
use crate::memory::sync::traits::{
    SkillDocument, SyncContext, SyncOutcome, SyncPipeline, SyncPipelineKind,
};

const ACTION_GET_ALL_TASKS: &str = "TODOIST_GET_ALL_TASKS";

/// Incremental Todoist synchronization through Composio.
///
/// Todoist tasks are self-contained records (stable id + `created_at`
/// timestamp), so this follows the document-shaped pattern
/// (`LinearSyncPipeline`) rather than the message-shaped one: a single list
/// action, content taken directly from the task payload with no secondary
/// fetch. Todoist's active-tasks endpoint returns a plain array and is not
/// paginated, so there is no server-side incremental filter, and a task carries
/// no modification timestamp. Incremental behavior is therefore driven by the
/// orchestrator's client-side dedup (`synced_ids`) keyed on a payload
/// fingerprint (see [`dedup_key`](Self::dedup_key)) — an unchanged task is
/// skipped, while any edit re-ingests.
pub struct TodoistSyncPipeline {
    client: ComposioClient,
    connection_id: String,
    max_pages: usize,
}

impl TodoistSyncPipeline {
    pub fn new(client: ComposioClient, connection_id: impl Into<String>) -> Self {
        Self {
            client,
            connection_id: connection_id.into(),
            max_pages: 1,
        }
    }

    pub fn with_limits(mut self, max_pages: usize, _page_size: usize) -> Self {
        self.max_pages = max_pages.max(1);
        // Todoist active-tasks is unpaginated; the sibling `page_size` argument
        // is accepted for signature parity but has no effect.
        self
    }
}

#[async_trait]
impl SyncPipeline for TodoistSyncPipeline {
    fn id(&self) -> &str {
        "composio:todoist"
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
impl IncrementalSource for TodoistSyncPipeline {
    fn toolkit(&self) -> &'static str {
        "todoist"
    }
    fn action(&self) -> &'static str {
        ACTION_GET_ALL_TASKS
    }
    fn max_pages(&self) -> usize {
        self.max_pages
    }
    fn stop_on_empty_pending(&self) -> bool {
        true
    }
    fn server_side_depth(&self) -> bool {
        false
    }
    fn arguments(
        &self,
        _: &SyncScope,
        _: &MemoryConfig,
        _: &SyncState,
        _page: Option<&str>,
    ) -> Value {
        // Todoist "get all active tasks" needs no required arguments and ignores
        // pagination; do not invent a page token.
        serde_json::json!({})
    }
    fn extract_page(&self, data: &Value, _: Option<&str>) -> PageFetch {
        PageFetch {
            items: first_array(
                data,
                &[
                    "/data/tasks",
                    "/tasks",
                    "/data/items",
                    "/items",
                    "/data",
                    "/data/data",
                ],
            ),
            // Todoist active tasks are returned as a single unpaginated array.
            next: None,
        }
    }
    fn dedup_key(&self, item: &Value) -> Option<String> {
        let id = pick_str(item, &["id", "data.id", "task_id", "data.task_id"])?;
        // Todoist tasks have no modification timestamp, so `created_at` (which is
        // immutable) would never change and edited tasks would never re-ingest.
        // Key on a fingerprint of the task payload instead: any change to
        // content/due/project yields a new key and re-ingests, while an
        // unchanged task keeps its key and is deduped.
        Some(format!("{id}@{}", payload_fingerprint(item)))
    }
    fn sort_cursor(&self, _item: &Value) -> Option<String> {
        // Todoist active tasks have no modification timestamp and the endpoint
        // is unpaginated, so there is no meaningful sort cursor. Returning None
        // is deliberate: the orchestrator's cursor-boundary short-circuit keys
        // on `sort_cursor`, and using the immutable `created_at` would halt the
        // scan (and skip re-ingest) for an edited task created before the
        // persisted cursor. Freshness is handled entirely by `dedup_key`.
        None
    }
    async fn document(
        &self,
        _: &SyncScope,
        connection_id: &str,
        item: SyncItem,
        _: &dyn ActionExecutor,
        _: &mut SyncState,
    ) -> anyhow::Result<SkillDocument> {
        let id = pick_str(&item.raw, &["id", "data.id", "task_id", "data.task_id"])
            .unwrap_or_else(|| item.dedup_key.clone());
        let title = pick_str(
            &item.raw,
            &["content", "data.content", "title", "data.title"],
        )
        .unwrap_or_else(|| format!("Todoist task {id}"));
        let content = serde_json::to_string_pretty(&item.raw)?;
        Ok(document(
            "todoist",
            connection_id,
            &id,
            title,
            content,
            item.raw,
        ))
    }
}

/// Stable content fingerprint of a task payload, used as the freshness half of
/// the dedup key. Deterministic for a given payload (`DefaultHasher` has a fixed
/// seed and `serde_json` serializes map keys in sorted order), so an unchanged
/// task always hashes the same and any edit changes the hash.
fn payload_fingerprint(item: &Value) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    item.to_string().hash(&mut hasher);
    hasher.finish()
}
