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
//!
//! ## Concurrency contract
//!
//! [`KvStore`], [`VectorStore`], and [`EntityIndex`] each hold their SQLite
//! connection behind a `parking_lot::Mutex`, so calls into one instance
//! serialize cleanly. Their SQLite connections use a 15-second busy timeout,
//! allowing independent handles to the same database file to tolerate brief
//! write contention instead of failing immediately with `SQLITE_BUSY`.

/// Markdown content store: source-of-truth files with YAML front matter,
/// atomic writes, and `content_path`/`content_sha256` provenance pointers.
pub mod content;
/// Entity occurrence index over tree nodes, backing co-occurrence graph queries.
pub mod entity_index;
/// Global + namespace-scoped JSON key-value store (writes pass the [`safety`] guard).
pub mod kv;
mod memory_trait;
/// Secret / PII detection guard applied before KV writes are persisted.
pub mod safety;
/// Starter in-memory reference backend used by the smoke test.
#[allow(clippy::module_inception)]
pub mod store;
/// Core memory contract types ([`StoreError`], [`MemoryRecord`], etc.).
pub mod types;
/// SQLite-backed packed-`f32` cosine vector DB and embedding backends.
pub mod vectors;

pub use store::{InMemoryMemoryStore, MemoryStore};
pub use types::{
    MemoryId, MemoryInput, MemoryQuery, MemoryRecord, MemoryResult, SearchHit, StoreError,
};

// ── Ported storage-primitive re-exports ─────────────────────────────────────
pub use entity_index::{CanonicalEntity, EntityHit, EntityIndex, EntityKind, SelfIdentity};
pub use kv::KvStore;
pub use vectors::{
    bytes_to_vec, cosine_similarity, vec_to_bytes, EmbeddingBackend, InertEmbedding, SearchResult,
    VectorStore,
};
