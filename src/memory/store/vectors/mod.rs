//! Local vector store (`VectorStore`) — SQLite-backed packed-`f32` cosine search.
//!
//! Ported from OpenHuman's `memory_store::vectors`. The model COMPUTE backend
//! is abstracted behind [`EmbeddingBackend`]; the store itself only persists
//! and searches vectors.

pub mod embedding;
pub mod store;

pub use embedding::{format_embedding_signature, EmbeddingBackend, InertEmbedding};
pub use store::{bytes_to_vec, cosine_similarity, vec_to_bytes, SearchResult, VectorStore};
