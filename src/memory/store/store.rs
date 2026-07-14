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
    MemoryId, MemoryInput, MemoryQuery, MemoryRecord, MemoryResult, SearchHit, StoreError,
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
    /// Fetch the record for `id`, or [`StoreError::NotFound`] if absent.
    async fn get(&self, id: MemoryId) -> MemoryResult<MemoryRecord>;
    /// Remove and return the record for `id`, or [`StoreError::NotFound`] if absent.
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
    pub(super) records: Arc<RwLock<BTreeMap<MemoryId, MemoryRecord>>>,
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
            .ok_or(StoreError::NotFound(id))
    }

    async fn delete(&self, id: MemoryId) -> MemoryResult<MemoryRecord> {
        self.records
            .write()
            .expect("memory store lock poisoned")
            .remove(&id)
            .ok_or(StoreError::NotFound(id))
    }

    async fn search(&self, query: MemoryQuery) -> MemoryResult<Vec<SearchHit>> {
        let query_text = query.text.as_deref().unwrap_or_default();
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
                    .is_none_or(|namespace| record.namespace == namespace)
            })
            .filter_map(|record| {
                let score = super::query_match_score(&record.content, query_text)?;

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
#[path = "store_tests.rs"]
mod tests;
