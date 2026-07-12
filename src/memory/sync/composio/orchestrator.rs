//! Shared bounded incremental synchronization control flow.

use async_trait::async_trait;
use serde_json::Value;

use super::client::ActionExecutor;
use crate::memory::config::MemoryConfig;
use crate::memory::sync::state::SyncState;
use crate::memory::sync::traits::{
    SkillDocument, SyncContext, SyncEvent, SyncOutcome, SyncRunError, SyncStage,
};

#[derive(Debug)]
pub struct PageFetch {
    pub items: Vec<Value>,
    pub next: Option<String>,
}

#[derive(Debug)]
pub struct SyncItem {
    pub dedup_key: String,
    pub sort_cursor: Option<String>,
    pub raw: Value,
}

#[derive(Clone, Debug, Default)]
pub struct SyncScope {
    pub id: String,
    pub label: String,
    pub metadata: Value,
}

impl SyncScope {
    pub fn flat() -> Self {
        Self::default()
    }

    pub fn named(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            metadata: Value::Null,
        }
    }

    pub fn with_metadata(mut self, metadata: Value) -> Self {
        self.metadata = metadata;
        self
    }
}

#[async_trait]
pub trait IncrementalSource: Send + Sync {
    fn toolkit(&self) -> &'static str;
    fn action(&self) -> &'static str;
    fn max_pages(&self) -> usize {
        10
    }
    fn per_scope_cursors(&self) -> bool {
        false
    }
    fn tolerate_scope_errors(&self) -> bool {
        false
    }
    fn retain_dedup_keys(&self) -> bool {
        true
    }
    fn server_side_depth(&self) -> bool {
        false
    }
    fn depth_floor(&self, config: &MemoryConfig, state: &SyncState) -> Option<String> {
        if state.cursor.is_some() {
            return None;
        }
        config.sync.budget.sync_depth_days.map(|days| {
            (chrono::Utc::now() - chrono::Duration::days(days as i64))
                .format("%Y-%m-%dT%H:%M:%SZ")
                .to_string()
        })
    }
    fn advance_scope_cursor(&self, _state: &mut SyncState, _scope: &SyncScope, _cursor: &str) {}
    async fn scopes(
        &self,
        _executor: &dyn ActionExecutor,
        _connection_id: &str,
        _state: &mut SyncState,
    ) -> anyhow::Result<Vec<SyncScope>> {
        Ok(vec![SyncScope::flat()])
    }
    fn arguments(
        &self,
        scope: &SyncScope,
        config: &MemoryConfig,
        state: &SyncState,
        page: Option<&str>,
    ) -> Value;
    fn extract_page(&self, data: &Value, page: Option<&str>) -> PageFetch;
    fn dedup_key(&self, item: &Value) -> Option<String>;
    fn sort_cursor(&self, item: &Value) -> Option<String>;
    async fn document(
        &self,
        scope: &SyncScope,
        connection_id: &str,
        item: SyncItem,
        executor: &dyn ActionExecutor,
        state: &mut SyncState,
    ) -> anyhow::Result<SkillDocument>;
}

pub async fn run_incremental_sync(
    source: &dyn IncrementalSource,
    executor: &dyn ActionExecutor,
    connection_id: &str,
    config: &MemoryConfig,
    context: &SyncContext,
) -> anyhow::Result<SyncOutcome> {
    let toolkit = source.toolkit();
    emit(context, toolkit, connection_id, SyncStage::Fetching, None).await;
    tracing::debug!(toolkit, connection_id, "[sync:orchestrator] sync starting");

    let mut state = SyncState::load(context.state.as_ref(), toolkit, connection_id).await?;
    if state.budget_exhausted() {
        tracing::debug!(
            toolkit,
            connection_id,
            "[sync:orchestrator] daily budget exhausted"
        );
        return Ok(SyncOutcome {
            note: Some("daily request budget exhausted".into()),
            ..SyncOutcome::default()
        });
    }

    let result = match source.scopes(executor, connection_id, &mut state).await {
        Ok(scopes) => {
            run_pages(
                source,
                executor,
                connection_id,
                config,
                context,
                &mut state,
                &scopes,
            )
            .await
        }
        Err(error) => Err(error),
    };
    state.last_sync_at_ms = Some(now_ms());
    if let Err(error) = state.save(context.state.as_ref()).await {
        emit(
            context,
            toolkit,
            connection_id,
            SyncStage::Failed,
            Some("sync state persistence failed".into()),
        )
        .await;
        return Err(error);
    }

    match result {
        Ok(outcome) => {
            emit(
                context,
                toolkit,
                connection_id,
                SyncStage::Stored,
                Some(format!("{} records", outcome.records_ingested)),
            )
            .await;
            emit(context, toolkit, connection_id, SyncStage::Completed, None).await;
            tracing::debug!(
                toolkit,
                connection_id,
                records = outcome.records_ingested,
                more_pending = outcome.more_pending,
                "[sync:orchestrator] sync completed"
            );
            Ok(outcome)
        }
        Err(error) => {
            tracing::warn!(toolkit, connection_id, %error, "[sync:orchestrator] sync failed");
            emit(
                context,
                toolkit,
                connection_id,
                SyncStage::Failed,
                Some(error.to_string()),
            )
            .await;
            Err(SyncRunError::new(
                error.to_string(),
                state.run_requests,
                state.run_provider_cost_usd,
            )
            .into())
        }
    }
}

async fn run_pages(
    source: &dyn IncrementalSource,
    executor: &dyn ActionExecutor,
    connection_id: &str,
    config: &MemoryConfig,
    context: &SyncContext,
    state: &mut SyncState,
    scopes: &[SyncScope],
) -> anyhow::Result<SyncOutcome> {
    let mut newest_cursor = state.cursor.clone();
    let mut ingested = 0u32;
    let mut more_pending = false;
    let depth_floor = (!source.server_side_depth())
        .then(|| source.depth_floor(config, state))
        .flatten();

    'scopes: for scope in scopes {
        let mut page_token = None;
        let mut scope_newest_cursor: Option<String> = None;
        let mut scope_failed = false;
        for page_index in 0..source.max_pages().max(1) {
            if state.budget_exhausted() {
                more_pending = true;
                break 'scopes;
            }
            let response = match executor
                .execute(
                    source.action(),
                    source.arguments(scope, config, state, page_token.as_deref()),
                    Some(connection_id),
                )
                .await
            {
                Ok(response) => response,
                Err(error) if source.tolerate_scope_errors() => {
                    if let Some(execute_error) = error.downcast_ref::<super::client::ExecuteError>()
                    {
                        state.record_requests(execute_error.attempts);
                    }
                    tracing::warn!(toolkit = source.toolkit(), connection_id, scope = %scope.label, %error, "[sync:orchestrator] scope fetch failed; continuing");
                    scope_failed = true;
                    break;
                }
                Err(error) => {
                    if let Some(execute_error) = error.downcast_ref::<super::client::ExecuteError>()
                    {
                        state.record_requests(execute_error.attempts);
                    }
                    return Err(error);
                }
            };
            // A completed provider round-trip is billable even when its envelope
            // reports failure. Transport failures return before this point.
            state.record_action(response.attempts, response.cost_usd);
            if !response.successful {
                let error = anyhow::anyhow!(
                    "{} provider failure: {}",
                    source.toolkit(),
                    response
                        .error
                        .unwrap_or_else(|| "unknown provider error".into())
                );
                if source.tolerate_scope_errors() {
                    tracing::warn!(toolkit = source.toolkit(), connection_id, scope = %scope.label, %error, "[sync:orchestrator] provider rejected scope; continuing");
                    scope_failed = true;
                    break;
                }
                return Err(error);
            }

            let fetched = source.extract_page(&response.data, page_token.as_deref());
            let mut reached_cursor_boundary = false;
            for raw in fetched.items {
                if config
                    .sync
                    .budget
                    .max_items
                    .is_some_and(|limit| ingested >= limit)
                {
                    more_pending = true;
                    break 'scopes;
                }
                if state.budget_exhausted() {
                    more_pending = true;
                    break 'scopes;
                }
                let Some(dedup_key) = source.dedup_key(&raw) else {
                    continue;
                };
                if state.is_synced(&dedup_key) {
                    continue;
                }
                let sort_cursor = source.sort_cursor(&raw);
                if sort_cursor
                    .as_deref()
                    .zip(depth_floor.as_deref())
                    .is_some_and(|(item_cursor, floor)| item_cursor < floor)
                {
                    reached_cursor_boundary = true;
                    break;
                }
                if !source.per_scope_cursors()
                    && sort_cursor
                        .as_deref()
                        .zip(state.cursor.as_deref())
                        .is_some_and(|(item_cursor, persisted_cursor)| {
                            item_cursor <= persisted_cursor
                        })
                {
                    tracing::debug!(
                        toolkit = source.toolkit(),
                        connection_id,
                        scope = %scope.label,
                        "[sync:orchestrator] reached persisted cursor boundary"
                    );
                    reached_cursor_boundary = true;
                    break;
                }
                let document = match source
                    .document(
                        scope,
                        connection_id,
                        SyncItem {
                            dedup_key: dedup_key.clone(),
                            sort_cursor: sort_cursor.clone(),
                            raw,
                        },
                        executor,
                        state,
                    )
                    .await
                {
                    Ok(document) => document,
                    Err(error) if source.tolerate_scope_errors() => {
                        tracing::warn!(toolkit = source.toolkit(), connection_id, scope = %scope.label, %error, "[sync:orchestrator] scope document conversion failed; continuing");
                        scope_failed = true;
                        break;
                    }
                    Err(error) => return Err(error),
                };
                if let Err(error) = context.documents.store(document).await {
                    if source.tolerate_scope_errors() {
                        tracing::warn!(toolkit = source.toolkit(), connection_id, scope = %scope.label, %error, "[sync:orchestrator] scope document store failed; continuing");
                        scope_failed = true;
                        break;
                    }
                    return Err(error);
                }
                if source.retain_dedup_keys() {
                    state.mark_synced(dedup_key);
                }
                if let Some(cursor) = sort_cursor {
                    let target = if source.per_scope_cursors() {
                        &mut scope_newest_cursor
                    } else {
                        &mut newest_cursor
                    };
                    if target
                        .as_deref()
                        .is_none_or(|current| cursor.as_str() > current)
                    {
                        *target = Some(cursor);
                    }
                }
                ingested = ingested.saturating_add(1);
                if config
                    .sync
                    .budget
                    .max_items
                    .is_some_and(|limit| ingested >= limit)
                {
                    more_pending = true;
                    break 'scopes;
                }
            }

            page_token = fetched.next;
            if reached_cursor_boundary {
                break;
            }
            if page_token.is_none() {
                break;
            }
            if page_index + 1 == source.max_pages().max(1) {
                more_pending = true;
            }
        }
        if source.per_scope_cursors() && !scope_failed && !more_pending {
            if let Some(cursor) = scope_newest_cursor.as_deref() {
                source.advance_scope_cursor(state, scope, cursor);
                state.save(context.state.as_ref()).await?;
            }
        }
    }

    if !source.per_scope_cursors() && !more_pending {
        if let Some(cursor) = newest_cursor {
            state.advance_cursor(cursor);
        }
    }
    Ok(SyncOutcome {
        records_ingested: ingested,
        more_pending,
        actions_called: state.run_requests,
        provider_cost_usd: state.run_provider_cost_usd,
        note: None,
    })
}

async fn emit(
    context: &SyncContext,
    toolkit: &str,
    connection_id: &str,
    stage: SyncStage,
    message: Option<String>,
) {
    let _ = context
        .events
        .emit(SyncEvent {
            source_id: format!("composio:{toolkit}:{connection_id}"),
            toolkit: toolkit.into(),
            connection_id: Some(connection_id.into()),
            stage,
            message,
        })
        .await;
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
