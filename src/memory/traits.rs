//! The high-level [`Memory`] trait every storage backend implements.
//!
//! Ported from OpenHuman's `memory::traits`. Backend-specific escape hatches
//! (e.g. raw SQLite connection access) are intentionally omitted here so the
//! trait stays storage-agnostic; concrete backends expose those via their own
//! inherent methods.

use async_trait::async_trait;

use super::types::{MemoryCategory, MemoryEntry, MemoryTaint, NamespaceSummary, RecallOpts};

/// The core trait for memory storage and retrieval.
///
/// Any persistence backend (SQLite, Postgres, vector DB, in-memory, …) should
/// implement this to participate in the TinyCortex memory engine.
#[async_trait]
pub trait Memory: Send + Sync {
    /// Returns the backend name (e.g. `"sqlite"`, `"vector"`, `"in_memory"`).
    fn name(&self) -> &str;

    /// Stores a new memory entry or updates an existing one.
    async fn store(
        &self,
        namespace: &str,
        key: &str,
        content: &str,
        category: MemoryCategory,
        session_id: Option<&str>,
    ) -> anyhow::Result<()>;

    /// Store an entry with explicit provenance taint.
    ///
    /// Sync paths ingesting third-party text MUST use this with
    /// [`MemoryTaint::ExternalSync`]. The default implementation degrades to
    /// [`Self::store`] for backends that do not yet persist taint.
    async fn store_with_taint(
        &self,
        namespace: &str,
        key: &str,
        content: &str,
        category: MemoryCategory,
        session_id: Option<&str>,
        taint: MemoryTaint,
    ) -> anyhow::Result<()> {
        let _ = taint;
        self.store(namespace, key, content, category, session_id)
            .await
    }

    /// Recalls memories matching a query using keyword or semantic search.
    async fn recall(
        &self,
        query: &str,
        limit: usize,
        opts: RecallOpts<'_>,
    ) -> anyhow::Result<Vec<MemoryEntry>>;

    /// Recall documents whose *vector* similarity alone meets a threshold.
    ///
    /// Returns `(key, content)` pairs, most-relevant first. Defaults to empty so
    /// keyword-only / mock backends opt out.
    async fn recall_relevant_by_vector(
        &self,
        namespace: &str,
        query: &str,
        limit: usize,
        min_vector_similarity: f64,
    ) -> anyhow::Result<Vec<(String, String)>> {
        let _ = (namespace, query, limit, min_vector_similarity);
        Ok(Vec::new())
    }

    /// Retrieves a specific entry by exact `(namespace, key)`.
    async fn get(&self, namespace: &str, key: &str) -> anyhow::Result<Option<MemoryEntry>>;

    /// Lists entries, optionally scoped by namespace, category, and session.
    async fn list(
        &self,
        namespace: Option<&str>,
        category: Option<&MemoryCategory>,
        session_id: Option<&str>,
    ) -> anyhow::Result<Vec<MemoryEntry>>;

    /// Deletes the entry for `(namespace, key)`. Returns whether it existed.
    async fn forget(&self, namespace: &str, key: &str) -> anyhow::Result<bool>;

    /// Lists all namespaces with aggregate stats for agent-side discovery.
    async fn namespace_summaries(&self) -> anyhow::Result<Vec<NamespaceSummary>>;

    /// Total count of all entries in the backend.
    async fn count(&self) -> anyhow::Result<usize>;

    /// Health check on the underlying storage system.
    async fn health_check(&self) -> bool;
}
