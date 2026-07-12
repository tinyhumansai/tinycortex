use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;

use async_trait::async_trait;
use chrono::Utc;
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

const ACTION_CHANNELS: &str = "SLACK_LIST_CONVERSATIONS";
const ACTION_HISTORY: &str = "SLACK_FETCH_CONVERSATION_HISTORY";
const ACTION_SEARCH: &str = "SLACK_SEARCH_MESSAGES";

pub struct SlackSyncPipeline {
    client: ComposioClient,
    connection_id: String,
    max_pages: usize,
    page_size: usize,
    backfill_days: i64,
}

pub struct SlackSearchBackfillPipeline {
    client: ComposioClient,
    connection_id: String,
    backfill_days: i64,
    max_pages: u32,
}

impl SlackSearchBackfillPipeline {
    pub fn new(
        client: ComposioClient,
        connection_id: impl Into<String>,
        backfill_days: i64,
    ) -> Self {
        Self {
            client,
            connection_id: connection_id.into(),
            backfill_days: backfill_days.max(1),
            max_pages: 50,
        }
    }
}

#[async_trait]
impl SyncPipeline for SlackSearchBackfillPipeline {
    fn id(&self) -> &str {
        "composio:slack:search-backfill"
    }

    fn kind(&self) -> SyncPipelineKind {
        SyncPipelineKind::Composio
    }

    async fn init(&self, _: &MemoryConfig, _: &SyncContext) -> anyhow::Result<()> {
        Ok(())
    }

    async fn tick(&self, _: &MemoryConfig, context: &SyncContext) -> anyhow::Result<SyncOutcome> {
        let mut state =
            SyncState::load(context.state.as_ref(), "slack", &self.connection_id).await?;
        if state.budget_exhausted() {
            return Ok(SyncOutcome {
                note: Some("slack search-backfill skipped: daily budget exhausted".into()),
                ..SyncOutcome::default()
            });
        }

        let directory = SlackSyncPipeline::new(self.client.clone(), self.connection_id.clone());
        let scopes = directory
            .scopes(&self.client, &self.connection_id, &mut state)
            .await?;
        let channels: HashMap<_, _> = scopes
            .into_iter()
            .map(|scope| (scope.id.clone(), scope))
            .collect();
        let users = channels
            .values()
            .find_map(|scope| scope.metadata.get("users").and_then(Value::as_object));
        let after = (Utc::now() - chrono::Duration::days(self.backfill_days))
            .format("%Y-%m-%d")
            .to_string();
        let mut page = 1u32;
        let mut total_pages = 1u32;
        let mut stored = 0u32;

        loop {
            if state.budget_exhausted() || page > self.max_pages {
                break;
            }
            let response = checked_execute(
                &self.client,
                ACTION_SEARCH,
                serde_json::json!({
                    "query": format!("after:{after}"),
                    "count": 100,
                    "sort": "timestamp",
                    "sort_dir": "asc",
                    "page": page,
                }),
                &self.connection_id,
                &mut state,
            )
            .await?;
            if page == 1 {
                total_pages = search_total_pages(&response.data).min(self.max_pages);
            }
            let matches = search_matches(&response.data);
            let fetched = matches.len();
            for raw in matches {
                let Some(ts) = pick_str(&raw, &["ts"]) else {
                    continue;
                };
                if parse_ts(&ts).is_none() {
                    continue;
                }
                let Some(text) = pick_str(&raw, &["text"]).filter(|text| !text.trim().is_empty())
                else {
                    continue;
                };
                let Some(channel_id) = pick_str(&raw, &["channel.id", "channel_id"]) else {
                    continue;
                };
                let Some(scope) = channels.get(&channel_id) else {
                    tracing::warn!(channel_id, "[sync:slack-search] unknown channel skipped");
                    continue;
                };
                let author_id =
                    pick_str(&raw, &["user", "bot_id"]).unwrap_or_else(|| "unknown".into());
                let author = users
                    .and_then(|users| users.get(&author_id))
                    .and_then(Value::as_str)
                    .unwrap_or(&author_id);
                let text = replace_mentions(&text, users);
                let mut doc = document(
                    "slack",
                    &self.connection_id,
                    &format!("{channel_id}:{ts}"),
                    format!("Slack {} from {author}", scope.label),
                    format!("[{ts}] {author}: {text}"),
                    raw,
                );
                doc.metadata["channel_id"] = Value::String(channel_id);
                doc.metadata["channel_label"] = Value::String(scope.label.clone());
                context.documents.store(doc).await?;
                stored = stored.saturating_add(1);
            }
            if fetched == 0 || page >= total_pages {
                break;
            }
            page = page.saturating_add(1);
        }

        state.last_sync_at_ms = Some(Utc::now().timestamp_millis() as u64);
        state.save(context.state.as_ref()).await?;
        Ok(SyncOutcome {
            records_ingested: stored,
            more_pending: page < total_pages,
            actions_called: state.run_requests,
            provider_cost_usd: state.run_provider_cost_usd,
            note: Some(format!(
                "slack search-backfill: pages={page} records={stored}"
            )),
        })
    }
}

impl SlackSyncPipeline {
    pub fn new(client: ComposioClient, connection_id: impl Into<String>) -> Self {
        Self {
            client,
            connection_id: connection_id.into(),
            max_pages: 20,
            page_size: 200,
            backfill_days: 30,
        }
    }
}

#[async_trait]
impl SyncPipeline for SlackSyncPipeline {
    fn id(&self) -> &str {
        "composio:slack"
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
impl IncrementalSource for SlackSyncPipeline {
    fn toolkit(&self) -> &'static str {
        "slack"
    }
    fn action(&self) -> &'static str {
        ACTION_HISTORY
    }
    fn max_pages(&self) -> usize {
        self.max_pages
    }
    fn per_scope_cursors(&self) -> bool {
        true
    }
    fn server_side_depth(&self) -> bool {
        true
    }
    fn tolerate_scope_errors(&self) -> bool {
        true
    }
    fn retain_dedup_keys(&self) -> bool {
        false
    }

    fn advance_scope_cursor(&self, state: &mut SyncState, scope: &SyncScope, cursor: &str) {
        let mut cursors = decode_cursors(state.cursor.as_deref());
        cursors.insert(scope.id.clone(), cursor.into());
        state.cursor = serde_json::to_string(&cursors).ok();
    }

    async fn scopes(
        &self,
        executor: &dyn ActionExecutor,
        connection_id: &str,
        state: &mut SyncState,
    ) -> anyhow::Result<Vec<SyncScope>> {
        let users = fetch_users(executor, connection_id, state).await;
        let mut cursor: Option<String> = None;
        let mut channels = Vec::new();
        for _ in 0..20 {
            if state.budget_exhausted() {
                break;
            }
            let mut args = serde_json::json!({"limit": 200, "types": "public_channel,private_channel", "exclude_archived": true});
            if let Some(cursor) = cursor.as_deref() {
                args["cursor"] = Value::String(cursor.into());
            }
            let response =
                checked_execute(executor, ACTION_CHANNELS, args, connection_id, state).await?;
            channels.extend(first_array(
                &response.data,
                &["/data/channels", "/channels", "/data/data/channels"],
            ));
            cursor = next_cursor(&response.data);
            if cursor.is_none() {
                break;
            }
        }
        Ok(channels
            .into_iter()
            .filter_map(|channel| {
                let id = pick_str(&channel, &["id", "data.id"])?;
                let name = pick_str(&channel, &["name", "data.name"]).unwrap_or_else(|| id.clone());
                let private = channel
                    .get("is_private")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let label = if private {
                    format!("private:{name}")
                } else {
                    format!("#{name}")
                };
                Some(
                    SyncScope::named(id, label).with_metadata(serde_json::json!({
                        "channel": channel,
                        "users": users,
                    })),
                )
            })
            .collect())
    }

    fn arguments(
        &self,
        scope: &SyncScope,
        config: &MemoryConfig,
        state: &SyncState,
        page: Option<&str>,
    ) -> Value {
        let cursors = decode_cursors(state.cursor.as_deref());
        let oldest = cursors.get(&scope.id).cloned().unwrap_or_else(|| {
            format!(
                "{}.000000",
                (Utc::now()
                    - chrono::Duration::days(
                        config
                            .sync
                            .budget
                            .sync_depth_days
                            .map(i64::from)
                            .unwrap_or(self.backfill_days)
                    ))
                .timestamp()
            )
        });
        let mut args = serde_json::json!({"channel": scope.id, "oldest": oldest, "inclusive": false, "limit": self.page_size});
        if let Some(page) = page {
            args["cursor"] = Value::String(page.into());
        }
        args
    }

    fn extract_page(&self, data: &Value, _: Option<&str>) -> PageFetch {
        PageFetch {
            items: first_array(
                data,
                &["/data/messages", "/messages", "/data/data/messages"],
            ),
            next: next_cursor(data),
        }
    }

    fn dedup_key(&self, item: &Value) -> Option<String> {
        let ts = pick_str(item, &["ts", "data.ts"])?;
        parse_ts(&ts)?;
        let text = pick_str(item, &["text", "data.text"])?;
        (!text.trim().is_empty()).then_some(ts)
    }

    fn sort_cursor(&self, item: &Value) -> Option<String> {
        pick_str(item, &["ts", "data.ts"])
    }

    async fn document(
        &self,
        scope: &SyncScope,
        connection_id: &str,
        item: SyncItem,
        _: &dyn ActionExecutor,
        _: &mut SyncState,
    ) -> anyhow::Result<SkillDocument> {
        let ts = pick_str(&item.raw, &["ts", "data.ts"]).unwrap_or(item.dedup_key);
        let raw_text = pick_str(&item.raw, &["text", "data.text"]).unwrap_or_default();
        let author_id = pick_str(
            &item.raw,
            &["user", "data.user", "username", "data.username"],
        )
        .unwrap_or_else(|| "unknown".into());
        let users = scope.metadata.get("users").and_then(Value::as_object);
        let author = users
            .and_then(|users| users.get(&author_id))
            .and_then(Value::as_str)
            .unwrap_or(&author_id)
            .to_owned();
        let text = replace_mentions(&raw_text, users);
        let title = format!("Slack {} from {}", scope.label, author);
        let content = format!("[{ts}] {author}: {text}");
        let mut result = document(
            "slack",
            connection_id,
            &format!("{}:{ts}", scope.id),
            title,
            content,
            item.raw,
        );
        result.metadata["channel_id"] = Value::String(scope.id.clone());
        result.metadata["channel_label"] = Value::String(scope.label.clone());
        Ok(result)
    }
}

async fn fetch_users(
    executor: &dyn ActionExecutor,
    connection_id: &str,
    state: &mut SyncState,
) -> HashMap<String, String> {
    let mut users = HashMap::new();
    let mut cursor: Option<String> = None;
    for page in 0..20 {
        if state.budget_exhausted() {
            break;
        }
        let mut arguments = serde_json::json!({"limit": 200});
        if let Some(cursor) = cursor.as_deref() {
            arguments["cursor"] = Value::String(cursor.into());
        }
        let response = match executor
            .execute("SLACK_LIST_ALL_USERS", arguments, Some(connection_id))
            .await
        {
            Ok(response) => response,
            Err(error) => {
                if let Some(error) = error.downcast_ref::<super::super::client::ExecuteError>() {
                    state.record_requests(error.attempts);
                }
                tracing::warn!(page, %error, "[sync:slack] user directory fetch failed; using collected users");
                break;
            }
        };
        state.record_action(response.attempts, response.cost_usd);
        if !response.successful {
            tracing::warn!(
                page,
                error = response.error.as_deref().unwrap_or("provider failure"),
                "[sync:slack] user directory rejected; using collected users"
            );
            break;
        }
        let members = first_array(
            &response.data,
            &[
                "/data/members",
                "/members",
                "/data/users",
                "/users",
                "/data/data/members",
            ],
        );
        for member in members {
            let Some(id) = pick_str(&member, &["id"]) else {
                continue;
            };
            if let Some(name) = [
                "profile.display_name",
                "profile.real_name",
                "real_name",
                "name",
                "profile.display_name_normalized",
                "profile.real_name_normalized",
            ]
            .iter()
            .find_map(|path| pick_str(&member, &[*path]))
            {
                users.insert(id, name);
            }
        }
        cursor = next_cursor(&response.data);
        if cursor.is_none() {
            break;
        }
    }
    users
}

fn mention_regex() -> &'static regex::Regex {
    static REGEX: OnceLock<regex::Regex> = OnceLock::new();
    REGEX.get_or_init(|| regex::Regex::new(r"<@(U[A-Z0-9]+)>").expect("Slack mention regex"))
}

fn replace_mentions(text: &str, users: Option<&serde_json::Map<String, Value>>) -> String {
    mention_regex()
        .replace_all(text, |captures: &regex::Captures<'_>| {
            let id = &captures[1];
            let resolved = users
                .and_then(|users| users.get(id))
                .and_then(Value::as_str)
                .unwrap_or(id);
            format!("@{resolved}")
        })
        .into_owned()
}

fn next_cursor(data: &Value) -> Option<String> {
    [
        "/data/response_metadata/next_cursor",
        "/response_metadata/next_cursor",
        "/data/next_cursor",
        "/next_cursor",
        "/data/data/response_metadata/next_cursor",
    ]
    .iter()
    .find_map(|path| data.pointer(path).and_then(Value::as_str))
    .map(str::trim)
    .filter(|cursor| !cursor.is_empty())
    .map(str::to_owned)
}

fn search_matches(data: &Value) -> Vec<Value> {
    first_array(
        data,
        &[
            "/data/messages/matches",
            "/messages/matches",
            "/data/data/messages/matches",
            "/messages",
        ],
    )
}

fn search_total_pages(data: &Value) -> u32 {
    [
        "/data/messages/paging/pages",
        "/messages/paging/pages",
        "/data/data/messages/paging/pages",
        "/pages",
    ]
    .iter()
    .find_map(|path| data.pointer(path).and_then(Value::as_u64))
    .unwrap_or(1) as u32
}

fn decode_cursors(raw: Option<&str>) -> BTreeMap<String, String> {
    raw.and_then(|raw| serde_json::from_str(raw).ok())
        .unwrap_or_default()
}

fn parse_ts(ts: &str) -> Option<(i64, u64)> {
    let mut parts = ts.splitn(2, '.');
    Some((
        parts.next()?.parse().ok()?,
        parts.next().unwrap_or("0").parse().ok()?,
    ))
}
