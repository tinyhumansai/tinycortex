use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;

use super::types::{
    MemoryError, MemoryId, MemoryInput, MemoryQuery, MemoryRecord, MemoryResult, SearchHit,
};

#[async_trait]
pub trait MemoryStore: Send + Sync {
    async fn insert(&self, input: MemoryInput) -> MemoryResult<MemoryRecord>;
    async fn get(&self, id: MemoryId) -> MemoryResult<MemoryRecord>;
    async fn delete(&self, id: MemoryId) -> MemoryResult<MemoryRecord>;
    async fn search(&self, query: MemoryQuery) -> MemoryResult<Vec<SearchHit>>;
}

#[derive(Clone, Debug, Default)]
pub struct InMemoryMemoryStore {
    records: Arc<RwLock<BTreeMap<MemoryId, MemoryRecord>>>,
}

impl InMemoryMemoryStore {
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
