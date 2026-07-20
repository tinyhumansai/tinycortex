//! Incremental Trello synchronization through Composio.
//!
//! Trello is a card-shaped source: each board is a [`SyncScope`], and cards on a
//! board are the syncable items. The pipeline models the ClickUp provider
//! (scope-per-container, global recency cursor) rather than Gmail, because
//! ingestion fans out over boards and pages cards within each one.
//!
//! Dedupe is keyed on the stable card id (`document_id = "trello:<card id>"`,
//! set by [`super::common::document`]); per-run identifiers are never used as the
//! upsert key. The dedup key additionally suffixes `dateLastActivity` so an
//! edited card re-ingests, mirroring the other incremental providers.

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

/// Boards visible to the authorized member (`idMember = "me"`).
const ACTION_BOARDS: &str = "TRELLO_GET_MEMBERS_BOARDS_BY_ID_MEMBER";
/// Cards on a single board — the per-scope fetch action.
const ACTION_CARDS: &str = "TRELLO_GET_BOARDS_CARDS_BY_ID_BOARD";

pub struct TrelloSyncPipeline {
    client: ComposioClient,
    connection_id: String,
    max_pages: usize,
    page_size: usize,
}

impl TrelloSyncPipeline {
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
impl SyncPipeline for TrelloSyncPipeline {
    fn id(&self) -> &str {
        "composio:trello"
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
impl IncrementalSource for TrelloSyncPipeline {
    fn toolkit(&self) -> &'static str {
        "trello"
    }
    fn action(&self) -> &'static str {
        ACTION_CARDS
    }
    fn max_pages(&self) -> usize {
        self.max_pages
    }
    /// Boards are independent scopes: an inaccessible or archived board must not
    /// abort ingestion for the rest.
    fn tolerate_scope_errors(&self) -> bool {
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
            ACTION_BOARDS,
            serde_json::json!({"idMember": "me"}),
            connection_id,
            state,
        )
        .await?;
        let boards = first_array(
            &response.data,
            &["/data", "/data/items", "/items", "/data/boards", "/boards"],
        );
        Ok(boards
            .into_iter()
            .filter_map(|board| pick_str(&board, &["id", "data.id"]))
            .map(|id| SyncScope::named(id.clone(), format!("board:{id}")))
            .collect())
    }
    fn arguments(
        &self,
        scope: &SyncScope,
        _: &MemoryConfig,
        _: &SyncState,
        page: Option<&str>,
    ) -> Value {
        // Trello paginates card listings with `before`, a card id that bounds the
        // window to cards older than it (ids are time-ordered).
        let mut args = serde_json::json!({"idBoard": scope.id, "limit": self.page_size});
        if let Some(page) = page {
            args["before"] = serde_json::json!(page);
        }
        args
    }
    fn extract_page(&self, data: &Value, _: Option<&str>) -> PageFetch {
        // Composio wraps the Trello REST array under `data`; tolerate the common
        // envelope shapes seen across toolkits.
        let items = first_array(
            data,
            &[
                "/data",
                "/data/items",
                "/items",
                "/data/data",
                "/data/cards",
                "/cards",
                "/data/results",
                "/results",
            ],
        );
        // Trello has no page token: request the next window with `before` set to
        // the oldest card id in this full page. A short page means the board is
        // drained.
        let next = (items.len() == self.page_size)
            .then(|| {
                items
                    .last()
                    .and_then(|card| pick_str(card, &["id", "data.id"]))
            })
            .flatten();
        PageFetch { items, next }
    }
    fn dedup_key(&self, item: &Value) -> Option<String> {
        let id = pick_str(item, &["id", "data.id"])?;
        Some(match self.sort_cursor(item) {
            Some(activity) => format!("{id}@{activity}"),
            None => id,
        })
    }
    fn sort_cursor(&self, item: &Value) -> Option<String> {
        pick_str(
            item,
            &[
                "dateLastActivity",
                "data.dateLastActivity",
                "date_last_activity",
                "data.date_last_activity",
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
        let id = pick_str(&item.raw, &["id", "data.id"]).unwrap_or_else(|| item.dedup_key.clone());
        let title = pick_str(&item.raw, &["name", "data.name", "title", "data.title"])
            .unwrap_or_else(|| format!("Trello card {id}"));
        let content = serde_json::to_string_pretty(&item.raw)?;
        let mut result = document("trello", connection_id, &id, title, content, item.raw);
        result.metadata["board_id"] = Value::String(scope.id.clone());
        Ok(result)
    }
}

#[cfg(test)]
#[path = "trello_tests.rs"]
mod tests;
