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
//! Use [`EntityIndex::for_memory_config`] for engine data. It wraps the chunk
//! store's shared connection, so scoring, retrieval, cascades, and this typed
//! facade all observe the same table without changing owner-managed pragmas.
//! [`EntityIndex::open`] remains available for a deliberately independent
//! standalone index.

pub mod store;
mod transaction;
pub mod types;

pub use store::{EntityIndex, NoSelfIdentity, SelfIdentity};
pub use transaction::{
    clear_entity_index_for_node_tx, index_entities_tx, index_entities_tx_with_identity,
    index_summary_entity_ids_tx_with_identity,
};
pub use types::{CanonicalEntity, EntityHit, EntityKind};
