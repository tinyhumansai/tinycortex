//! The backend seam the harness measures.
//!
//! [`BenchBackend`] is the minimal contract every measured memory system must
//! satisfy: ingest a document, then answer a query with a ranked list of the
//! document ids it would surface. Keeping this abstract means the harness can
//! score the simple [`InMemoryMemoryStore`] today and a fully assembled
//! `CortexEngine` (goal C1) or a live-embedding backend later, unchanged.

use async_trait::async_trait;
use serde_json::json;
use tinycortex::memory::{InMemoryMemoryStore, MemoryInput, MemoryQuery, MemoryStore};

use crate::dataset::Document;

/// Metadata key under which a backend stashes the dataset document id so it can
/// be recovered from a retrieval hit (store ids are backend-minted UUIDs).
const DOC_ID_KEY: &str = "bench_doc_id";

/// A memory system under evaluation.
#[async_trait]
pub trait BenchBackend {
    /// Short identifier recorded in the run report (e.g. `"in_memory_store"`).
    fn name(&self) -> &str;

    /// Ingest one document so it becomes retrievable.
    async fn ingest(&self, doc: &Document) -> anyhow::Result<()>;

    /// Retrieve up to `k` documents for `text`, returning their dataset ids in
    /// rank order (most relevant first). `namespace` scopes the search when
    /// `Some`; `None` searches across all namespaces.
    async fn query(
        &self,
        namespace: Option<&str>,
        text: &str,
        k: usize,
    ) -> anyhow::Result<Vec<String>>;
}

/// [`BenchBackend`] adapter over the crate's reference [`InMemoryMemoryStore`].
///
/// Ingest stores each document's dataset id in record metadata; queries map the
/// store's lexical [`SearchHit`](tinycortex::SearchHit)s back to those ids. This
/// exercises the lexical/keyword retrieval path only — an inert baseline that
/// the graph/vector/tree backends must beat.
pub struct InMemoryBackend {
    store: InMemoryMemoryStore,
}

impl InMemoryBackend {
    /// Create an empty in-memory backend.
    pub fn new() -> Self {
        Self {
            store: InMemoryMemoryStore::new(),
        }
    }
}

impl Default for InMemoryBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl BenchBackend for InMemoryBackend {
    fn name(&self) -> &str {
        "in_memory_store"
    }

    async fn ingest(&self, doc: &Document) -> anyhow::Result<()> {
        let mut input = MemoryInput::new(doc.namespace.clone(), doc.text.clone());
        input.metadata.insert(DOC_ID_KEY.to_string(), json!(doc.id));
        if !doc.title.is_empty() {
            input.metadata.insert("title".to_string(), json!(doc.title));
        }
        self.store.insert(input).await?;
        Ok(())
    }

    async fn query(
        &self,
        namespace: Option<&str>,
        text: &str,
        k: usize,
    ) -> anyhow::Result<Vec<String>> {
        let hits = self
            .store
            .search(MemoryQuery {
                namespace: namespace.map(str::to_string),
                text: Some(text.to_string()),
                limit: Some(k),
            })
            .await?;

        Ok(hits
            .into_iter()
            .filter_map(|hit| {
                hit.record
                    .metadata
                    .get(DOC_ID_KEY)
                    .and_then(|value| value.as_str())
                    .map(str::to_string)
            })
            .collect())
    }
}
