//! Shared test infrastructure for the tool-scoped memory layer.
//!
//! Provides a minimal in-memory [`Memory`] backend so the store tests can
//! run without depending on an (un-ported) SQLite/file store. Only compiled
//! under `#[cfg(test)]`.

use std::collections::HashMap;

use async_trait::async_trait;
use parking_lot::Mutex;

use crate::memory::traits::Memory;
use crate::memory::types::{MemoryCategory, MemoryEntry, NamespaceSummary, RecallOpts};

/// Minimal in-memory [`Memory`] backend for unit tests.
///
/// Stores entries in a `HashMap` keyed by `(namespace, key)`. Methods that
/// are not needed by the store tests are simple no-ops; category/session
/// filters are intentionally ignored so tests focus on caller behavior
/// rather than backend indexing.
#[derive(Default)]
pub struct MockMemory {
    pub entries: Mutex<HashMap<(String, String), MemoryEntry>>,
}

#[async_trait]
impl Memory for MockMemory {
    fn name(&self) -> &str {
        "mock"
    }

    async fn store(
        &self,
        namespace: &str,
        key: &str,
        content: &str,
        category: MemoryCategory,
        session_id: Option<&str>,
    ) -> anyhow::Result<()> {
        self.entries.lock().insert(
            (namespace.to_string(), key.to_string()),
            MemoryEntry {
                id: format!("{namespace}/{key}"),
                key: key.to_string(),
                content: content.to_string(),
                namespace: Some(namespace.to_string()),
                category,
                timestamp: "now".into(),
                session_id: session_id.map(str::to_string),
                score: None,
                taint: Default::default(),
            },
        );
        Ok(())
    }

    async fn recall(
        &self,
        _query: &str,
        _limit: usize,
        _opts: RecallOpts<'_>,
    ) -> anyhow::Result<Vec<MemoryEntry>> {
        Ok(Vec::new())
    }

    async fn get(&self, namespace: &str, key: &str) -> anyhow::Result<Option<MemoryEntry>> {
        Ok(self
            .entries
            .lock()
            .get(&(namespace.to_string(), key.to_string()))
            .cloned())
    }

    async fn list(
        &self,
        namespace: Option<&str>,
        _category: Option<&MemoryCategory>,
        _session_id: Option<&str>,
    ) -> anyhow::Result<Vec<MemoryEntry>> {
        let lock = self.entries.lock();
        Ok(match namespace {
            Some(ns) => lock
                .iter()
                .filter(|((n, _), _)| n == ns)
                .map(|(_, v)| v.clone())
                .collect(),
            None => lock.values().cloned().collect(),
        })
    }

    async fn forget(&self, namespace: &str, key: &str) -> anyhow::Result<bool> {
        Ok(self
            .entries
            .lock()
            .remove(&(namespace.to_string(), key.to_string()))
            .is_some())
    }

    async fn namespace_summaries(&self) -> anyhow::Result<Vec<NamespaceSummary>> {
        let mut counts: HashMap<String, usize> = HashMap::new();
        for ((ns, _), _) in self.entries.lock().iter() {
            *counts.entry(ns.clone()).or_default() += 1;
        }
        Ok(counts
            .into_iter()
            .map(|(namespace, count)| NamespaceSummary {
                namespace,
                count,
                last_updated: None,
            })
            .collect())
    }

    async fn count(&self) -> anyhow::Result<usize> {
        Ok(self.entries.lock().len())
    }

    async fn health_check(&self) -> bool {
        true
    }
}

#[cfg(test)]
#[path = "test_helpers_tests.rs"]
mod tests;
