//! Shared bounded incremental synchronization control flow.

use async_trait::async_trait;
use serde_json::Value;

use super::client::ActionExecutor;
use super::page_size::{apply_page_size, is_payload_too_large, shrink_page_size};
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
    fn stop_on_empty_pending(&self) -> bool {
        false
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
    /// Name of the argument that caps how many items one page requests
    /// (`max_results` for Gmail), when the action has one.
    ///
    /// Returning `Some` opts the source into the too-large-page retry: a page
    /// the provider refuses purely for size is re-requested with the cap
    /// halved, instead of failing the whole run. A source whose action has no
    /// such knob returns `None` and keeps the previous behaviour.
    fn page_size_arg_key(&self) -> Option<&'static str> {
        None
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

    // Once a page proves too large for the provider, every later page of this
    // run asks for the smaller size straight away rather than paying a rejected
    // round-trip to rediscover the same limit.
    let mut page_size_override: Option<u64> = None;

    'scopes: for scope in scopes {
        let mut page_token = None;
        let mut scope_newest_cursor: Option<String> = None;
        let mut scope_failed = false;
        'pages: for page_index in 0..source.max_pages().max(1) {
            if state.budget_exhausted() {
                more_pending = true;
                break 'scopes;
            }
            let mut arguments = source.arguments(scope, config, state, page_token.as_deref());
            apply_page_size(
                &mut arguments,
                source.page_size_arg_key(),
                page_size_override,
            );
            // The size a shrink most recently *tried*. Promoted to the run's
            // sticky override only once the provider accepts a page at it —
            // before that it names a size that may itself be refused.
            let mut attempted_page_size: Option<u64> = None;
            let response = loop {
                let response = match executor
                    .execute(source.action(), arguments.clone(), Some(connection_id))
                    .await
                {
                    Ok(response) => response,
                    Err(error) if source.tolerate_scope_errors() => {
                        if let Some(execute_error) =
                            error.downcast_ref::<super::client::ExecuteError>()
                        {
                            state.record_requests(execute_error.attempts);
                        }
                        tracing::warn!(toolkit = source.toolkit(), connection_id, scope = %scope.label, %error, "[sync:orchestrator] scope fetch failed; continuing");
                        scope_failed = true;
                        break 'pages;
                    }
                    Err(error) => {
                        if let Some(execute_error) =
                            error.downcast_ref::<super::client::ExecuteError>()
                        {
                            state.record_requests(execute_error.attempts);
                        }
                        return Err(error);
                    }
                };
                // A completed provider round-trip is billable even when its
                // envelope reports failure. Transport failures return before
                // this point.
                state.record_action(response.attempts, response.cost_usd);
                if response.successful {
                    // Sticky for the rest of the run, and set HERE rather than
                    // at the shrink: with several halvings (25 → 12 → 6) an
                    // assignment per attempt leaves the last *rejected* size in
                    // the override on every step but the final one, and is
                    // correct at the end only because that step happens to be
                    // the accepted one. Recording the accepted size makes the
                    // intent independent of the retry order.
                    if let Some(accepted) = attempted_page_size {
                        page_size_override = Some(accepted);
                    }
                    break response;
                }
                // A page refused purely for its size is the one provider
                // failure a *smaller request* can fix, so shrink and retry
                // instead of failing the run. Without this a single oversized
                // page stops the source dead until someone notices: on one live
                // workspace Gmail sync sat broken for nine days that way.
                if is_payload_too_large(response.error.as_deref()) {
                    if state.budget_exhausted() {
                        more_pending = true;
                        break 'scopes;
                    }
                    if let Some(reduced) =
                        shrink_page_size(&mut arguments, source.page_size_arg_key())
                    {
                        attempted_page_size = Some(reduced);
                        tracing::warn!(
                            toolkit = source.toolkit(),
                            connection_id,
                            scope = %scope.label,
                            reduced_page_size = reduced,
                            "[sync:orchestrator] provider refused the page as too large; retrying with a smaller page"
                        );
                        continue;
                    }
                }
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
                    break 'pages;
                }
                return Err(error);
            };

            let fetched = source.extract_page(&response.data, page_token.as_deref());
            let mut reached_cursor_boundary = false;
            let mut saw_unsynced_item = false;
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
                saw_unsynced_item = true;
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
            if source.stop_on_empty_pending() && !saw_unsynced_item {
                tracing::debug!(
                    toolkit = source.toolkit(),
                    connection_id,
                    scope = %scope.label,
                    "[sync:orchestrator] stopping after all-deduplicated page"
                );
                break;
            }
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

#[cfg(test)]
#[path = "orchestrator_tests.rs"]
mod tests;
