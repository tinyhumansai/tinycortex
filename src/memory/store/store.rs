//! Minimal [`MemoryStore`] contract and an in-process reference implementation.
//!
//! Defines the CRUD-plus-search surface that storage backends must satisfy and
//! ships [`InMemoryMemoryStore`], a volatile `BTreeMap`-backed store used by
//! tests and as the simplest conforming backend. Records are keyed by
//! [`MemoryId`] and scoped by a free-form `namespace` string; durability and the
//! authoritative markdown vault live in higher layers.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;

use super::types::{
    MemoryError, MemoryId, MemoryInput, MemoryQuery, MemoryRecord, MemoryResult, SearchHit,
};

/// Storage backend contract for memory records.
///
/// Implementations provide insert/get/delete plus namespace- and text-scoped
/// search over [`MemoryRecord`]s. Required to be `Send + Sync` so a single store
/// can be shared across async tasks.
#[async_trait]
pub trait MemoryStore: Send + Sync {
    /// Materialize a record from `input` and persist it, returning the stored
    /// record (with its assigned [`MemoryId`] and timestamps).
    async fn insert(&self, input: MemoryInput) -> MemoryResult<MemoryRecord>;
    /// Fetch the record for `id`, or [`MemoryError::NotFound`] if absent.
    async fn get(&self, id: MemoryId) -> MemoryResult<MemoryRecord>;
    /// Remove and return the record for `id`, or [`MemoryError::NotFound`] if absent.
    async fn delete(&self, id: MemoryId) -> MemoryResult<MemoryRecord>;
    /// Return scored [`SearchHit`]s matching `query`, ordered most relevant first.
    async fn search(&self, query: MemoryQuery) -> MemoryResult<Vec<SearchHit>>;
}

/// Volatile, in-process [`MemoryStore`] backed by a `BTreeMap`.
///
/// Holds records under an `Arc<RwLock<..>>` so the store is cheaply cloneable
/// and shareable across tasks while serializing mutations. Contents are lost on
/// drop; intended for tests and as the simplest conforming backend, not for
/// durable storage.
#[derive(Clone, Debug, Default)]
pub struct InMemoryMemoryStore {
    /// Records indexed by [`MemoryId`]; the `BTreeMap` keeps a stable key order.
    records: Arc<RwLock<BTreeMap<MemoryId, MemoryRecord>>>,
}

impl InMemoryMemoryStore {
    /// Create an empty store.
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl MemoryStore for InMemoryMemoryStore {
    async fn insert(&self, input: MemoryInput) -> MemoryResult<MemoryRecord> {
        let record = MemoryRecord::from_input(input)?;
        self.records
            .write()
            .expect("memory store lock poisoned")
            .insert(record.id, record.clone());
        Ok(record)
    }

    async fn get(&self, id: MemoryId) -> MemoryResult<MemoryRecord> {
        self.records
            .read()
            .expect("memory store lock poisoned")
            .get(&id)
            .cloned()
            .ok_or(MemoryError::NotFound(id))
    }

    async fn delete(&self, id: MemoryId) -> MemoryResult<MemoryRecord> {
        self.records
            .write()
            .expect("memory store lock poisoned")
            .remove(&id)
            .ok_or(MemoryError::NotFound(id))
    }

    async fn search(&self, query: MemoryQuery) -> MemoryResult<Vec<SearchHit>> {
        let needle = query.text.as_deref().map(str::to_lowercase);
        let limit = query.limit.unwrap_or(20);

        let mut hits = self
            .records
            .read()
            .expect("memory store lock poisoned")
            .values()
            .filter(|record| {
                query
                    .namespace
                    .as_deref()
                    .map_or(true, |namespace| record.namespace == namespace)
            })
            .filter_map(|record| {
                let score = match needle.as_deref() {
                    Some(text) if !text.is_empty() => {
                        let content = record.content.to_lowercase();
                        if !content.contains(text) {
                            return None;
                        }
                        text.split_whitespace()
                            .filter(|term| content.contains(term))
                            .count()
                            .max(1) as f32
                    }
                    _ => 1.0,
                };

                Some(SearchHit {
                    record: record.clone(),
                    score,
                })
            })
            .collect::<Vec<_>>();

        hits.sort_by(|left, right| {
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| right.record.created_at.cmp(&left.record.created_at))
        });
        hits.truncate(limit);
        Ok(hits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn inserts_and_retrieves_memory() {
        let store = InMemoryMemoryStore::new();
        let inserted = store
            .insert(MemoryInput::new("profile", "User prefers dark mode"))
            .await
            .expect("insert memory");

        let fetched = store.get(inserted.id).await.expect("get memory");

        assert_eq!(fetched.namespace, "profile");
        assert_eq!(fetched.content, "User prefers dark mode");
    }

    #[tokio::test]
    async fn searches_by_namespace_and_text() {
        let store = InMemoryMemoryStore::new();
        store
            .insert(MemoryInput::new("profile", "User prefers dark mode"))
            .await
            .expect("insert profile memory");
        store
            .insert(MemoryInput::new(
                "project",
                "TinyCortex stores durable memories",
            ))
            .await
            .expect("insert project memory");

        let hits = store
            .search(MemoryQuery {
                namespace: Some("project".to_owned()),
                text: Some("durable".to_owned()),
                limit: Some(10),
            })
            .await
            .expect("search memory");

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].record.namespace, "project");
    }
}
