//! Persistence-neutral cursor, deduplication, and daily-budget state.

use std::collections::{HashMap, HashSet};

use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};

pub const DEFAULT_DAILY_REQUEST_LIMIT: u32 = 500;
pub const STATE_NAMESPACE: &str = "composio-sync-state";

#[async_trait]
pub trait SyncStateStore: Send + Sync {
    async fn get(&self, namespace: &str, key: &str) -> anyhow::Result<Option<serde_json::Value>>;
    async fn set(
        &self,
        namespace: &str,
        key: &str,
        value: &serde_json::Value,
    ) -> anyhow::Result<()>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyBudget {
    pub date: String,
    pub requests_used: u32,
    pub limit: u32,
}

impl Default for DailyBudget {
    fn default() -> Self {
        Self {
            date: today(),
            requests_used: 0,
            limit: DEFAULT_DAILY_REQUEST_LIMIT,
        }
    }
}

impl DailyBudget {
    pub fn remaining(&self) -> u32 {
        if self.date != today() {
            self.limit
        } else {
            self.limit.saturating_sub(self.requests_used)
        }
    }

    pub fn is_exhausted(&self) -> bool {
        self.remaining() == 0
    }

    pub fn record_requests(&mut self, count: u32) {
        let today = today();
        if self.date != today {
            self.date = today;
            self.requests_used = 0;
        }
        self.requests_used = self.requests_used.saturating_add(count);
    }

    pub fn record_request(&mut self) {
        self.record_requests(1);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncState {
    pub toolkit: String,
    pub connection_id: String,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub synced_ids: HashSet<String>,
    #[serde(default)]
    pub item_versions: HashMap<String, String>,
    #[serde(default)]
    pub daily_budget: DailyBudget,
    #[serde(default)]
    pub last_seen_id: Option<String>,
    #[serde(default)]
    pub last_sync_at_ms: Option<u64>,
    #[serde(skip)]
    pub run_requests: u32,
    #[serde(skip)]
    pub run_provider_cost_usd: f64,
}

impl SyncState {
    pub fn new(toolkit: impl Into<String>, connection_id: impl Into<String>) -> Self {
        Self {
            toolkit: toolkit.into(),
            connection_id: connection_id.into(),
            cursor: None,
            synced_ids: HashSet::new(),
            item_versions: HashMap::new(),
            daily_budget: DailyBudget::default(),
            last_seen_id: None,
            last_sync_at_ms: None,
            run_requests: 0,
            run_provider_cost_usd: 0.0,
        }
    }

    pub fn key(toolkit: &str, connection_id: &str) -> String {
        format!("{toolkit}:{connection_id}")
    }

    pub fn is_synced(&self, id: &str) -> bool {
        self.synced_ids.contains(id)
    }

    pub fn mark_synced(&mut self, id: impl Into<String>) {
        self.synced_ids.insert(id.into());
    }

    pub fn advance_cursor(&mut self, cursor: impl Into<String>) {
        self.cursor = Some(cursor.into());
    }

    pub fn set_last_seen_id(&mut self, id: impl Into<String>) {
        self.last_seen_id = Some(id.into());
    }

    pub fn set_last_sync_at_ms(&mut self, timestamp_ms: u64) {
        self.last_sync_at_ms = Some(timestamp_ms);
    }

    pub fn budget_exhausted(&self) -> bool {
        self.daily_budget.is_exhausted()
    }

    pub fn budget_remaining(&self) -> u32 {
        self.daily_budget.remaining()
    }

    pub fn record_requests(&mut self, count: u32) {
        self.daily_budget.record_requests(count);
        self.run_requests = self.run_requests.saturating_add(count);
    }

    pub fn record_action(&mut self, attempts: u32, cost_usd: f64) {
        self.record_requests(attempts.max(1));
        if cost_usd.is_finite() && cost_usd > 0.0 {
            self.run_provider_cost_usd += cost_usd;
        }
    }

    pub async fn load(
        store: &dyn SyncStateStore,
        toolkit: &str,
        connection_id: &str,
    ) -> anyhow::Result<Self> {
        let key = Self::key(toolkit, connection_id);
        match store.get(STATE_NAMESPACE, &key).await? {
            Some(value) => {
                let mut state: Self = serde_json::from_value(value)?;
                if state.daily_budget.date != today() {
                    state.daily_budget.date = today();
                    state.daily_budget.requests_used = 0;
                }
                Ok(state)
            }
            None => Ok(Self::new(toolkit, connection_id)),
        }
    }

    pub async fn save(&self, store: &dyn SyncStateStore) -> anyhow::Result<()> {
        let value = serde_json::to_value(self)?;
        store
            .set(
                STATE_NAMESPACE,
                &Self::key(&self.toolkit, &self.connection_id),
                &value,
            )
            .await
    }
}

fn today() -> String {
    Utc::now().format("%Y-%m-%d").to_string()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    struct MemoryStateStore(Mutex<HashMap<String, serde_json::Value>>);

    #[async_trait]
    impl SyncStateStore for MemoryStateStore {
        async fn get(
            &self,
            namespace: &str,
            key: &str,
        ) -> anyhow::Result<Option<serde_json::Value>> {
            Ok(self
                .0
                .lock()
                .unwrap()
                .get(&format!("{namespace}:{key}"))
                .cloned())
        }

        async fn set(
            &self,
            namespace: &str,
            key: &str,
            value: &serde_json::Value,
        ) -> anyhow::Result<()> {
            self.0
                .lock()
                .unwrap()
                .insert(format!("{namespace}:{key}"), value.clone());
            Ok(())
        }
    }

    #[tokio::test]
    async fn state_round_trips_cursor_dedup_and_budget() {
        let store = MemoryStateStore::default();
        let mut state = SyncState::new("gmail", "conn-1");
        state.advance_cursor("cursor-2");
        state.mark_synced("message-1");
        state.record_requests(3);
        state.save(&store).await.unwrap();

        let loaded = SyncState::load(&store, "gmail", "conn-1").await.unwrap();
        assert_eq!(loaded.cursor.as_deref(), Some("cursor-2"));
        assert!(loaded.is_synced("message-1"));
        assert_eq!(loaded.daily_budget.requests_used, 3);
    }

    #[test]
    fn stale_budget_reports_full_and_resets_on_record() {
        let mut budget = DailyBudget {
            date: "2000-01-01".into(),
            requests_used: 499,
            limit: 500,
        };
        assert_eq!(budget.remaining(), 500);
        budget.record_requests(1);
        assert_eq!(budget.requests_used, 1);
        assert_eq!(budget.remaining(), 499);
    }
}
