//! Embedding compute backend abstraction for the local vector store.
//!
//! The [`VectorStore`](super::store::VectorStore) only *persists* and *searches*
//! packed `f32` vectors. It never talks to a model directly — instead it holds
//! an [`EmbeddingBackend`] that turns text into vectors. This keeps the storage
//! primitive free of any model / network dependency: production hosts plug a
//! real backend (Ollama, Voyage, OpenAI, …) while tests use the inert
//! [`InertEmbedding`] which returns deterministic zero vectors of a fixed
//! dimension.
//!
//! Ported from OpenHuman's `embeddings::provider_trait::EmbeddingProvider`,
//! trimmed to the compute surface the vector store actually calls.

use async_trait::async_trait;

/// Formats the canonical embedding-space signature string.
///
/// This is the single source of truth for the signature format so a signature
/// computed from configuration is byte-identical to one computed from an
/// instantiated backend. Drift between the two would silently split one
/// embedding space into two.
pub fn format_embedding_signature(name: &str, model_id: &str, dims: usize) -> String {
    format!("provider={name};model={model_id};dims={dims}")
}

/// Interface for embedding backends that convert text into numerical vectors.
///
/// Implementors are the only place that performs model / network calls; the
/// vector store consumes the produced `f32` vectors and owns persistence and
/// cosine search.
#[async_trait]
pub trait EmbeddingBackend: Send + Sync {
    /// Short backend name (e.g. `"ollama"`, `"inert"`).
    fn name(&self) -> &str;

    /// Stable model identifier used to generate embeddings.
    fn model_id(&self) -> &str;

    /// Number of dimensions in the generated embeddings.
    fn dimensions(&self) -> usize;

    /// Stable signature for the embedding space.
    ///
    /// Changing any component means existing vectors may no longer be
    /// comparable with newly generated vectors and should be stored / queried
    /// separately by a follow-up storage migration.
    fn signature(&self) -> String {
        format_embedding_signature(self.name(), self.model_id(), self.dimensions())
    }

    /// Generate embeddings for a batch of strings.
    async fn embed(&self, texts: &[&str]) -> anyhow::Result<Vec<Vec<f32>>>;

    /// Generate an embedding for a single string.
    async fn embed_one(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        let mut results = self.embed(&[text]).await?;
        results
            .pop()
            .ok_or_else(|| anyhow::anyhow!("empty embedding result"))
    }
}

/// An inert backend that returns deterministic all-zero vectors of a fixed
/// dimension. Used by tests and by hosts that want keyword-only behaviour
/// without wiring a model. Dimension defaults to
/// [`DEFAULT_EMBEDDING_DIM`](crate::memory::config::DEFAULT_EMBEDDING_DIM).
#[derive(Clone, Debug)]
pub struct InertEmbedding {
    dims: usize,
}

impl InertEmbedding {
    /// Construct an inert backend producing `dims`-length zero vectors.
    pub fn new(dims: usize) -> Self {
        Self { dims }
    }
}

impl Default for InertEmbedding {
    fn default() -> Self {
        Self::new(crate::memory::config::DEFAULT_EMBEDDING_DIM)
    }
}

#[async_trait]
impl EmbeddingBackend for InertEmbedding {
    fn name(&self) -> &str {
        "inert"
    }

    fn model_id(&self) -> &str {
        "inert"
    }

    fn dimensions(&self) -> usize {
        self.dims
    }

    async fn embed(&self, texts: &[&str]) -> anyhow::Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|_| vec![0.0_f32; self.dims]).collect())
    }
}
