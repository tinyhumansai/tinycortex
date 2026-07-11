//! Memory graph — entity relationships derived from the tree entity index.
//!
//! The premise: a separate triple store is redundant when every chunk already
//! lands an entity row in the occurrence index (`mem_tree_entity_index`). The
//! graph IS the tree mapped out — two entities co-occurring on the same node
//! form an edge, and the edge weight is the count of distinct shared nodes.
//!
//! This module derives those edges on demand instead of writing a parallel
//! storage table. The LLM-extracted `(subject, predicate, object)` triple
//! surface is intentionally out of scope here (see [`crate::memory::types::GraphRelationRecord`]
//! for that separate, persisted shape).
//!
//! ## Decoupling
//!
//! The occurrence index lives in a different module
//! (`memory_store::entities`). Rather than hard-depending on its concrete
//! storage, the queries read through the [`EntityOccurrenceIndex`] trait and
//! take any implementation by injection. Production wires in the SQLite-backed
//! adapter; tests inject a small in-memory fixture. The graph itself owns no
//! state and performs no writes.
//!
//! ## API
//!
//! - [`co_occurring_entities`] — for a subject entity, every other entity that
//!   has appeared on a shared node, with a co-occurrence weight.
//! - [`neighbors`] — convenience: just the entity ids, no weights.
//! - [`group_by_weight`] — bucket edges by weight for strong-vs-weak rendering.
//!
//! ## Layout
//!
//! - [`types`]: [`GraphEdge`] and the [`EntityOccurrenceIndex`] read contract.
//! - [`query`]: the co-occurrence derivation and its helpers.
//!
//! ## Concurrency and cost
//!
//! This module holds no state and performs no writes, so there is nothing to
//! lock here — every call is a pure read against whatever
//! [`EntityOccurrenceIndex`] the caller injects, and repeated calls with the
//! same index contents are idempotent. The derivation itself is **not** O(1):
//! [`co_occurring_entities`] issues one `entities_on_node` lookup per node the
//! subject appears on, so its cost scales with the subject's occurrence count,
//! not a fixed query budget. Callers on hot paths (e.g. per-request retrieval)
//! should cap `limit` and be mindful of the underlying index's per-call cost.

pub mod query;
pub mod types;

pub use query::{co_occurring_entities, group_by_weight, neighbors};
pub use types::{EntityOccurrenceIndex, GraphEdge};
