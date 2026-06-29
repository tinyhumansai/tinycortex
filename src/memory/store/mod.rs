//! Memory contracts and storage primitives.
//!
//! The starter in-memory store (`store` / `types`) provides the simple
//! reference backend used by the smoke test. The richer storage primitives
//! ported from OpenHuman live in the submodules below:
//!
//! - [`content`]       — markdown content store (YAML front-matter, atomic
//!   writes, `content_path`/`content_sha256` pointers, Obsidian-vault paths).
//! - [`vectors`]       — SQLite-backed packed-`f32` cosine vector DB.
//! - [`kv`]            — global + namespace JSON key-value store.
//! - [`entity_index`] — entity occurrence index over tree nodes.
//! - [`safety`]       — secret / PII detection guard used by KV writes.

pub mod content;
pub mod entity_index;
pub mod kv;
pub mod safety;
pub mod store;
pub mod types;
pub mod vectors;

pub use store::{InMemoryMemoryStore, MemoryStore};
pub use types::{
    MemoryError, MemoryId, MemoryInput, MemoryQuery, MemoryRecord, MemoryResult, SearchHit,
};

// ── Ported storage-primitive re-exports ─────────────────────────────────────
pub use entity_index::{CanonicalEntity, EntityHit, EntityIndex, EntityKind, SelfIdentity};
pub use kv::KvStore;
pub use vectors::{
    bytes_to_vec, cosine_similarity, vec_to_bytes, EmbeddingBackend, InertEmbedding, SearchResult,
    VectorStore,
};
