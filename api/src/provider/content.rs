//! Optional families that put content *into* memory and navigate it:
//! [`MemoryIngest`], [`MemoryDocuments`], and [`MemoryTree`].
//!
//! All three are optional. A driver that advertises none of them is still a
//! memory backend — it just accepts entries only through
//! [`crate::provider::MemoryCore::store`] and has no document tier and no
//! summary tree. The kernel unregisters the matching RPC methods and omits the
//! matching agent tools rather than registering handlers that fail.
//!
//! ## No configuration crosses this boundary
//!
//! Chunk sizes, embedding models, summariser prompts, seal thresholds, and
//! cascade policy are all *driver* concerns. None of them appear in these
//! signatures: the embedded driver reads them from the `MemoryConfig` it
//! already holds, and an external driver has its own. This was the sharpest
//! test of whether the M0 crate carve-out drew the line in the right place —
//! the families that looked most config-dependent turned out not to need any.

use async_trait::async_trait;

use crate::chunks::Chunk;
use crate::error::MemoryError;
use crate::provider::types::{IngestItem, IngestOutcome, SourceScope};
use crate::tree::{IngestRequest, QueryResult, TreeStatus};
use crate::types::{NamespaceDocumentInput, NamespaceRetrievalContext, StoredMemoryDocument};

/// Bulk content ingestion — the driver owns chunking and embedding.
///
/// The distinction from [`crate::provider::MemoryCore::store`] is ownership of
/// the pipeline: `store` persists exactly one entry the caller has already
/// shaped, whereas ingest hands over raw source material and lets the driver
/// decide how to split, embed, and index it.
#[async_trait]
pub trait MemoryIngest: Send + Sync {
    /// Ingest one standalone document.
    ///
    /// # Errors
    ///
    /// [`MemoryError::Invalid`] for content the driver refuses (empty body,
    /// unsupported MIME), otherwise backend failures.
    async fn ingest_document(&self, item: IngestItem) -> Result<IngestOutcome, MemoryError>;

    /// Ingest a run of chat messages that share a conversation.
    ///
    /// Taken as a batch rather than one call per message because chat chunking
    /// is inherently cross-message: a driver needs neighbouring turns to decide
    /// where a chunk boundary belongs. Ordering within `messages` is
    /// significant and must be preserved by the caller.
    ///
    /// # Errors
    ///
    /// As [`Self::ingest_document`]. Partial success is reported through the
    /// counts in [`IngestOutcome`], not as an error.
    async fn ingest_chat(&self, messages: Vec<IngestItem>) -> Result<IngestOutcome, MemoryError>;
}

/// The namespace-document tier: whole documents addressed by `(namespace, key)`.
///
/// Distinct from [`crate::provider::MemoryCore`] in granularity and in what is
/// stored: entries are short facts, documents are bodies with titles, tags,
/// source types, and structured metadata, and they carry their own ranked query
/// surface.
#[async_trait]
pub trait MemoryDocuments: Send + Sync {
    /// Upsert a document, returning its driver-assigned id.
    ///
    /// Keyed by `(namespace, key)` from the input: reusing a key replaces the
    /// existing document rather than creating a second one.
    ///
    /// # Errors
    ///
    /// [`MemoryError::Invalid`] for a rejected input, otherwise backend
    /// failures.
    async fn put_document(&self, input: NamespaceDocumentInput) -> Result<String, MemoryError>;

    /// Fetch a document by `(namespace, key)`.
    ///
    /// # Errors
    ///
    /// A missing document is `Ok(None)`; `Err` is reserved for backend
    /// failures.
    async fn get_document(
        &self,
        namespace: &str,
        key: &str,
    ) -> Result<Option<StoredMemoryDocument>, MemoryError>;

    /// Run a ranked query over one namespace's documents.
    ///
    /// Returns both the ranked hits and the driver's rendered context text, so
    /// a caller that only wants something injectable does not have to
    /// re-assemble it (and re-assemble it differently from every other caller).
    ///
    /// # Errors
    ///
    /// Backend failures only; a query that matches nothing returns an empty
    /// hit list.
    async fn query_documents(
        &self,
        namespace: &str,
        query: &str,
        limit: usize,
    ) -> Result<NamespaceRetrievalContext, MemoryError>;
}

/// The time-ordered summary tree: buffered leaves rolled up into hour → day →
/// month → year → root summaries.
///
/// Sealing and cascading are exposed as explicit calls rather than happening
/// implicitly on ingest because the **host** owns scheduling. A driver runs one
/// step when asked; it does not get to install its own background loop. This is
/// the same rule as the engine's `queue::run_once`.
#[async_trait]
pub trait MemoryTree: Send + Sync {
    /// Append raw content to the ingestion buffer for later sealing.
    ///
    /// # Errors
    ///
    /// [`MemoryError::Invalid`] for a rejected request, otherwise backend
    /// failures.
    async fn append(&self, request: IngestRequest) -> Result<(), MemoryError>;

    /// Retrieve the chunks a single logical source contributed, newest first.
    ///
    /// `scope` is the per-turn allowlist and must be applied **inside** the
    /// driver's query, for the reasons in [`SourceScope`]. `None` means
    /// unrestricted.
    ///
    /// # Errors
    ///
    /// Backend failures only; an unknown `source_id` yields an empty vector.
    async fn query_source(
        &self,
        namespace: &str,
        source_id: &str,
        limit: usize,
        scope: Option<&SourceScope>,
    ) -> Result<Vec<Chunk>, MemoryError>;

    /// Fetch one node together with its direct children, for navigation.
    ///
    /// # Errors
    ///
    /// [`MemoryError::NotFound`] when `node_id` does not exist in `namespace`.
    async fn drill_down(&self, namespace: &str, node_id: &str) -> Result<QueryResult, MemoryError>;

    /// Convert buffered content into leaf nodes, returning the resulting tree
    /// state.
    ///
    /// Idempotent when the buffer is empty: sealing nothing is a successful
    /// no-op, not an error, so a scheduler may call it unconditionally.
    ///
    /// # Errors
    ///
    /// Backend failures only.
    async fn seal(&self, namespace: &str) -> Result<TreeStatus, MemoryError>;

    /// Roll sealed leaves up through the parent levels, returning the resulting
    /// tree state.
    ///
    /// Idempotent for the same reason as [`Self::seal`].
    ///
    /// # Errors
    ///
    /// Backend failures only.
    async fn cascade(&self, namespace: &str) -> Result<TreeStatus, MemoryError>;
}
