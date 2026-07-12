//! Entity occurrence index — the `mem_tree_entity_index` inverted index.
//!
//! Maps a canonical entity id to every tree node (chunk or summary) it appears
//! in, so retrieval can resolve entity-scoped queries in O(lookup). This is the
//! occurrence index over tree nodes, NOT the markdown contact registry.
//!
//! Ported from OpenHuman's `memory_tree::score::store` (entity CRUD) plus the
//! `EntityKind` / `CanonicalEntity` / `EntityHit` types from the score module.
//!
//! ## Concurrency / atomicity contract
//!
//! [`EntityIndex::open`] runs `PRAGMA journal_mode = WAL` unconditionally on
//! whatever `db_path` it is given. The chunk store (`crate::memory::chunks`)
//! declares this same `mem_tree_entity_index` table in its own schema and
//! deliberately enforces `TRUNCATE` journal mode on its database file. Callers
//! must therefore either (a) point `EntityIndex` at a dedicated database file
//! distinct from the chunk store's — accepting that the two copies of the
//! table are then independent and cascade-deletes / coverage counts on one
//! side won't see the other's rows — or (b) share the chunk store's
//! connection/path deliberately and accept that the WAL pragma here will flip
//! that database out of `TRUNCATE` mode. Neither option is handled
//! automatically by this module; picking one and documenting it is the
//! caller's responsibility.

pub mod store;
pub mod types;

pub use store::{
    clear_entity_index_for_node_tx, index_entities_tx, index_entities_tx_with_identity,
    index_summary_entity_ids_tx_with_identity, EntityIndex, NoSelfIdentity, SelfIdentity,
};
pub use types::{CanonicalEntity, EntityHit, EntityKind};
