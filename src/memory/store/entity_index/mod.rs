//! Entity occurrence index — the `mem_tree_entity_index` inverted index.
//!
//! Maps a canonical entity id to every tree node (chunk or summary) it appears
//! in, so retrieval can resolve entity-scoped queries in O(lookup). This is the
//! occurrence index over tree nodes, NOT the markdown contact registry.
//!
//! Ported from OpenHuman's `memory_tree::score::store` (entity CRUD) plus the
//! `EntityKind` / `CanonicalEntity` / `EntityHit` types from the score module.

pub mod store;
pub mod types;

pub use store::{index_entities_tx, EntityIndex, NoSelfIdentity, SelfIdentity};
pub use types::{CanonicalEntity, EntityHit, EntityKind};
