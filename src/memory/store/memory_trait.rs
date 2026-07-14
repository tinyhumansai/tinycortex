//! High-level [`Memory`](crate::memory::Memory) implementation for the
//! in-process reference backend.

use std::collections::HashMap;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde_json::Value;

use super::store::InMemoryMemoryStore;
use super::types::{MemoryInput, MemoryRecord};
use crate::memory::traits::Memory;
use crate::memory::types::{
    MemoryCategory, MemoryEntry, MemoryTaint, NamespaceSummary, RecallOpts, GLOBAL_NAMESPACE,
};

const KEY: &str = "tinycortex.key";
const CATEGORY: &str = "tinycortex.category";
const SESSION: &str = "tinycortex.session_id";
const TAINT: &str = "tinycortex.taint";

fn text_field<'a>(record: &'a MemoryRecord, name: &str) -> Option<&'a str> {
    record.metadata.get(name).and_then(Value::as_str)
}

fn entry_from_record(record: &MemoryRecord) -> MemoryEntry {
    let category = text_field(record, CATEGORY)
        .and_then(|value| value.parse().ok())
        .unwrap_or(MemoryCategory::Core);
    MemoryEntry {
        id: record.id.to_string(),
        key: text_field(record, KEY)
            .map(str::to_owned)
            .unwrap_or_else(|| record.id.to_string()),
        content: record.content.clone(),
        namespace: Some(record.namespace.clone()),
        category,
        timestamp: record.updated_at.to_rfc3339(),
        session_id: text_field(record, SESSION).map(str::to_owned),
        score: None,
        taint: MemoryTaint::from_db_str(text_field(record, TAINT).unwrap_or("external_sync")),
    }
}

fn query_score(content: &str, query: &str) -> Option<f64> {
    super::query_match_score(content, query).map(f64::from)
}

#[async_trait]
impl Memory for InMemoryMemoryStore {
    fn name(&self) -> &str {
        "in_memory"
    }

    async fn store(
        &self,
        namespace: &str,
        key: &str,
        content: &str,
        category: MemoryCategory,
        session_id: Option<&str>,
    ) -> Result<()> {
        self.store_with_taint(
            namespace,
            key,
            content,
            category,
            session_id,
            MemoryTaint::Internal,
        )
        .await
    }

    async fn store_with_taint(
        &self,
        namespace: &str,
        key: &str,
        content: &str,
        category: MemoryCategory,
        session_id: Option<&str>,
        taint: MemoryTaint,
    ) -> Result<()> {
        if key.trim().is_empty() {
            return Err(anyhow!("memory key cannot be empty"));
        }
        let content = content.trim();
        if content.is_empty() {
            return Err(anyhow!("memory content cannot be empty"));
        }
        let mut records = self
            .records
            .write()
            .map_err(|_| anyhow!("memory store lock poisoned"))?;
        let existing_id = records
            .values()
            .find(|record| record.namespace == namespace && text_field(record, KEY) == Some(key))
            .map(|record| record.id);
        let mut metadata = serde_json::Map::new();
        metadata.insert(KEY.into(), Value::String(key.to_string()));
        metadata.insert(CATEGORY.into(), Value::String(category.to_string()));
        if let Some(session_id) = session_id {
            metadata.insert(SESSION.into(), Value::String(session_id.to_string()));
        }
        metadata.insert(TAINT.into(), Value::String(taint.as_db_str().to_string()));

        if let Some(id) = existing_id {
            let record = records.get_mut(&id).expect("id came from the same map");
            record.content = content.to_string();
            record.metadata = metadata;
            record.updated_at = chrono::Utc::now();
        } else {
            let record = MemoryRecord::from_input(MemoryInput {
                namespace: namespace.to_string(),
                content: content.to_string(),
                metadata,
            })?;
            records.insert(record.id, record);
        }
        Ok(())
    }

    async fn recall(
        &self,
        query: &str,
        limit: usize,
        opts: RecallOpts<'_>,
    ) -> Result<Vec<MemoryEntry>> {
        let namespace = opts.namespace.unwrap_or(GLOBAL_NAMESPACE);
        let records = self
            .records
            .read()
            .map_err(|_| anyhow!("memory store lock poisoned"))?;
        let mut entries = records
            .values()
            .filter(|record| record.namespace == namespace)
            .map(entry_from_record)
            .filter(|entry| {
                opts.category
                    .as_ref()
                    .is_none_or(|category| entry.category == *category)
            })
            .filter(|entry| {
                opts.session_id.is_none_or(|session| {
                    entry.session_id.as_deref() == Some(session)
                        || (opts.cross_session && entry.category == MemoryCategory::Conversation)
                })
            })
            .filter_map(|mut entry| {
                let score = query_score(&entry.content, query)?;
                if opts.min_score.is_some_and(|minimum| score < minimum) {
                    return None;
                }
                entry.score = Some(score);
                Some(entry)
            })
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| {
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| right.timestamp.cmp(&left.timestamp))
                .then_with(|| left.key.cmp(&right.key))
        });
        entries.truncate(limit);
        Ok(entries)
    }

    async fn get(&self, namespace: &str, key: &str) -> Result<Option<MemoryEntry>> {
        let records = self
            .records
            .read()
            .map_err(|_| anyhow!("memory store lock poisoned"))?;
        Ok(records
            .values()
            .find(|record| record.namespace == namespace && text_field(record, KEY) == Some(key))
            .map(entry_from_record))
    }

    async fn list(
        &self,
        namespace: Option<&str>,
        category: Option<&MemoryCategory>,
        session_id: Option<&str>,
    ) -> Result<Vec<MemoryEntry>> {
        let records = self
            .records
            .read()
            .map_err(|_| anyhow!("memory store lock poisoned"))?;
        let mut entries = records
            .values()
            .filter(|record| namespace.is_none_or(|value| record.namespace == value))
            .map(entry_from_record)
            .filter(|entry| category.is_none_or(|value| entry.category == *value))
            .filter(|entry| {
                session_id.is_none_or(|value| entry.session_id.as_deref() == Some(value))
            })
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| left.key.cmp(&right.key));
        Ok(entries)
    }

    async fn forget(&self, namespace: &str, key: &str) -> Result<bool> {
        let mut records = self
            .records
            .write()
            .map_err(|_| anyhow!("memory store lock poisoned"))?;
        let id = records
            .values()
            .find(|record| record.namespace == namespace && text_field(record, KEY) == Some(key))
            .map(|record| record.id);
        Ok(id.and_then(|id| records.remove(&id)).is_some())
    }

    async fn namespace_summaries(&self) -> Result<Vec<NamespaceSummary>> {
        let records = self
            .records
            .read()
            .map_err(|_| anyhow!("memory store lock poisoned"))?;
        let mut summaries: HashMap<String, (usize, chrono::DateTime<chrono::Utc>)> = HashMap::new();
        for record in records.values() {
            let summary = summaries
                .entry(record.namespace.clone())
                .or_insert((0, record.updated_at));
            summary.0 += 1;
            summary.1 = summary.1.max(record.updated_at);
        }
        let mut summaries = summaries
            .into_iter()
            .map(|(namespace, (count, updated))| NamespaceSummary {
                namespace,
                count,
                last_updated: Some(updated.to_rfc3339()),
            })
            .collect::<Vec<_>>();
        summaries.sort_by(|left, right| left.namespace.cmp(&right.namespace));
        Ok(summaries)
    }

    async fn count(&self) -> Result<usize> {
        self.records
            .read()
            .map(|records| records.len())
            .map_err(|_| anyhow!("memory store lock poisoned"))
    }

    async fn health_check(&self) -> bool {
        self.records.read().is_ok()
    }
}

#[cfg(test)]
#[path = "memory_trait_tests.rs"]
mod tests;
