//! The high-level [`Memory`] trait every storage backend implements.
//!
//! Ported from OpenHuman's `memory::traits`. Backend-specific escape hatches
//! (e.g. raw SQLite connection access) are intentionally omitted here so the
//! trait stays storage-agnostic; concrete backends expose those via their own
//! inherent methods.
//!
//! ## Contract notes
//!
//! - Every method returns `anyhow::Result<_>` rather than a typed error: this
//!   trait is a stable abstraction boundary over heterogeneous backends
//!   (SQLite, vector DB, in-memory, …), each with its own error domain, so
//!   callers should treat a returned `Err` as opaque and log/propagate it
//!   rather than match on its variant. Concrete backends document their own
//!   failure modes (e.g. IO errors, malformed persisted rows) alongside their
//!   inherent methods.
//! - None of these methods are specified to panic; a conforming implementation
//!   should convert failures (invalid input, backend errors, poisoned locks)
//!   into `Err` instead.
//! - [`Memory::store`] and [`Memory::store_with_taint`] are upserts keyed by
//!   `(namespace, key)`: calling them again with the same key replaces the
//!   prior entry rather than erroring or duplicating it.

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
    ///
    /// Idempotent upsert keyed by `(namespace, key)`: calling this again with
    /// the same `namespace`/`key` replaces the previous `content`, `category`,
    /// and `session_id` rather than erroring or creating a duplicate. Entries
    /// stored this way carry [`MemoryTaint::Internal`] (the default); use
    /// [`Self::store_with_taint`] to persist content from an external source.
    ///
    /// # Errors
    ///
    /// Returns `Err` on any backend failure (IO, serialization, connection
    /// loss); implementations must not panic on caller-controlled input.
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
    /// [`Self::store`] for backends that do not yet persist taint — meaning it
    /// silently drops the `taint` argument for any backend that has not
    /// overridden this method. Backends whose durability/policy story depends
    /// on taint being recorded MUST override this method rather than rely on
    /// the default.
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
    ///
    /// `limit` caps the number of returned entries; `opts` narrows the search
    /// by namespace, category, session, minimum score, and cross-session
    /// inclusion (see [`RecallOpts`]). An empty or non-matching `query` should
    /// yield `Ok(vec![])`, not an error. Result ordering is backend-defined
    /// (typically most-relevant first) but callers must not assume a stable
    /// order across backends.
    async fn recall(
        &self,
        query: &str,
        limit: usize,
        opts: RecallOpts<'_>,
    ) -> anyhow::Result<Vec<MemoryEntry>>;

    /// Recall documents whose *vector* similarity alone meets a threshold.
    ///
    /// Returns `(key, content)` pairs, most-relevant first. Defaults to empty so
    /// keyword-only / mock backends opt out; a backend that overrides this
    /// should treat `min_vector_similarity` as an inclusive floor (hits scoring
    /// strictly below it are dropped) and `limit` as a hard cap on the
    /// returned count.
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
    ///
    /// Returns `Ok(None)` — not `Err` — when no entry exists for the pair;
    /// `Err` is reserved for backend failures.
    async fn get(&self, namespace: &str, key: &str) -> anyhow::Result<Option<MemoryEntry>>;

    /// Lists entries, optionally scoped by namespace, category, and session.
    ///
    /// Each `Option` filter narrows the result set when `Some`; passing all
    /// three as `None` lists every entry the backend holds. An empty result
    /// set is `Ok(vec![])`, never an error.
    async fn list(
        &self,
        namespace: Option<&str>,
        category: Option<&MemoryCategory>,
        session_id: Option<&str>,
    ) -> anyhow::Result<Vec<MemoryEntry>>;

    /// Deletes the entry for `(namespace, key)`. Returns whether it existed.
    ///
    /// Idempotent: forgetting an already-absent `(namespace, key)` returns
    /// `Ok(false)` rather than erroring, so callers may call this
    /// unconditionally without checking existence first.
    async fn forget(&self, namespace: &str, key: &str) -> anyhow::Result<bool>;

    /// Lists all namespaces with aggregate stats for agent-side discovery.
    ///
    /// See [`NamespaceSummary`] for the per-namespace count and
    /// last-updated timestamp returned.
    async fn namespace_summaries(&self) -> anyhow::Result<Vec<NamespaceSummary>>;

    /// Total count of all entries in the backend, across all namespaces.
    async fn count(&self) -> anyhow::Result<usize>;

    /// Health check on the underlying storage system.
    ///
    /// Returns `true` when the backend is reachable and able to serve
    /// requests. Unlike the other methods this reports failure as `false`
    /// rather than `Err`, so it is safe to call from a liveness probe without
    /// error-handling boilerplate.
    async fn health_check(&self) -> bool;
}
