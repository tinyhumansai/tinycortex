//! Embedding layer — fixed-dimension vectors for semantic rerank.
//!
//! Produces a fixed-dimension vector per chunk / summary so retrieval can
//! rerank candidates by semantic similarity. The backend is abstracted behind
//! the [`Embedder`] trait; the crate ships only the deterministic
//! [`InertEmbedder`] (zero vectors) used by tests. Real network-backed backends
//! (Ollama / OpenAI-compatible / cloud) are wired in by a host adapter that
//! implements [`Embedder`] — TinyCortex never makes a network call.
//!
//! Dimension is fixed at [`EMBEDDING_DIM`] (768, from
//! [`crate::memory::config::DEFAULT_EMBEDDING_DIM`]) — mixing dimensions
//! mid-run would corrupt cosine comparisons, so we catch that at the trait
//! level rather than deferring to retrieval-time diagnostics.

use anyhow::{Context, Result};
use async_trait::async_trait;

/// Embedding dimensionality used across the memory tree.
///
/// Fixed to the OpenHuman default (768). Any change breaks on-disk
/// compatibility with existing embedding blobs.
pub const EMBEDDING_DIM: usize = crate::memory::config::DEFAULT_EMBEDDING_DIM;

/// Trait backing all embedders. Implementations MUST produce exactly
/// [`EMBEDDING_DIM`] floats per call — callers that persist the result rely on
/// the fixed layout.
#[async_trait]
pub trait Embedder: Send + Sync {
    /// Stable short name, used in debug logs and provider diagnostics.
    fn name(&self) -> &'static str;

    /// Embed one text. Must return a `Vec<f32>` of length [`EMBEDDING_DIM`].
    /// Hard failure — ingest / seal treat `Err` as "don't persist the row" so
    /// retries stay idempotent on `chunk_id`.
    async fn embed(&self, text: &str) -> Result<Vec<f32>>;

    /// Embed many texts, returning **one [`Result`] per input position**
    /// aligned by index. A single failing text does not strand the rest of the
    /// batch — its slot carries the `Err` while the others succeed.
    ///
    /// The default implementation issues one sequential [`Embedder::embed`]
    /// call per text. Providers whose backend accepts many texts in one request
    /// override this to collapse N round-trips into one.
    async fn embed_batch(&self, texts: &[&str]) -> Vec<Result<Vec<f32>>> {
        let mut out = Vec::with_capacity(texts.len());
        for text in texts {
            out.push(self.embed(text).await);
        }
        out
    }
}

/// Validate that a freshly-produced embedding has exactly [`EMBEDDING_DIM`]
/// floats, returning a labelled error otherwise.
pub fn check_embed_dim(v: Vec<f32>, label: &str) -> Result<Vec<f32>> {
    if v.len() != EMBEDDING_DIM {
        anyhow::bail!(
            "{label} embedder returned {} dims, expected {}",
            v.len(),
            EMBEDDING_DIM
        );
    }
    Ok(v)
}

/// Cosine similarity between two equal-length vectors.
///
/// Returns `0.0` when either vector has zero magnitude (including empty
/// vectors) to keep the rerank sort stable instead of surfacing `NaN`. Length
/// mismatch also returns `0.0`. Otherwise returns the raw cosine value in
/// `[-1.0, 1.0]` — unlike
/// [`crate::memory::store::vectors::store::cosine_similarity`] (`f64`,
/// clamped to `[0.0, 1.0]`), anti-correlated vectors here yield a negative
/// score rather than `0.0`.
///
/// NOTE: this crate has two independent `cosine_similarity` implementations
/// (this one and the `f64`, `[0,1]`-clamped one used by MMR reranking in
/// `store::vectors::store`) that disagree on sign and precision. Callers that
/// mix results from both call sites cannot reliably compare them; prefer
/// consolidating on one before adding a third caller.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0_f32;
    let mut na = 0.0_f32;
    let mut nb = 0.0_f32;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

/// Pack a `Vec<f32>` into little-endian bytes for SQLite BLOB storage. Output
/// length is `v.len() * 4`. The inverse is [`unpack_embedding`].
pub fn pack_embedding(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for f in v {
        out.extend_from_slice(&f.to_le_bytes());
    }
    out
}

/// Unpack little-endian bytes into a `Vec<f32>`.
///
/// Errors when the byte length isn't a multiple of 4 or doesn't match
/// [`EMBEDDING_DIM`] (after decoding) — the latter guards against rows written
/// with a mismatched-provider blob silently passing as valid.
pub fn unpack_embedding(b: &[u8]) -> Result<Vec<f32>> {
    if !b.len().is_multiple_of(4) {
        anyhow::bail!(
            "embedding blob length {} not a multiple of 4 — corrupt row",
            b.len()
        );
    }
    let floats: Vec<f32> = b
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    if floats.len() != EMBEDDING_DIM {
        anyhow::bail!(
            "embedding blob length {} floats, expected {}",
            floats.len(),
            EMBEDDING_DIM
        );
    }
    Ok(floats)
}

/// Pack helper that also validates the input dimension before storing. Used by
/// write-time call sites where we want a loud error if a provider misbehaves
/// rather than writing a differently-shaped blob.
pub fn pack_checked(v: &[f32]) -> Result<Vec<u8>> {
    if v.len() != EMBEDDING_DIM {
        anyhow::bail!(
            "embedding vector has {} dims, expected {}",
            v.len(),
            EMBEDDING_DIM
        );
    }
    Ok(pack_embedding(v))
}

/// Decode a possibly-NULL embedding blob straight from a query row. Returns
/// `Ok(None)` for NULL (legacy rows predating embeddings) and surfaces decoding
/// errors with context so the caller sees which row was malformed.
pub fn decode_optional_blob(
    blob: Option<Vec<u8>>,
    context_label: &str,
) -> Result<Option<Vec<f32>>> {
    match blob {
        None => Ok(None),
        Some(bytes) => {
            let v = unpack_embedding(&bytes)
                .with_context(|| format!("decode embedding for {context_label}"))?;
            Ok(Some(v))
        }
    }
}

/// Deterministic zero-vector embedder for tests.
///
/// `embed` always returns a fresh `Vec<f32>` of length [`EMBEDDING_DIM`] filled
/// with zeros — no network, no randomness. Note: because every chunk/summary
/// ends up with the same zero vector, cosine similarity between them is always
/// `0.0` (see [`cosine_similarity`]). Tests that want reranking should stitch
/// embeddings via store accessors rather than rely on the inert path.
#[derive(Clone, Copy, Debug, Default)]
pub struct InertEmbedder;

impl InertEmbedder {
    /// Construct an inert embedder. Free — `InertEmbedder` is a ZST.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Embedder for InertEmbedder {
    fn name(&self) -> &'static str {
        "inert"
    }

    async fn embed(&self, _text: &str) -> Result<Vec<f32>> {
        Ok(vec![0.0; EMBEDDING_DIM])
    }
}

#[cfg(test)]
#[path = "embed_tests.rs"]
mod tests;
