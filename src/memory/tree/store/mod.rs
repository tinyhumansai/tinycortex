//! SQLite-backed persistence for summary trees — ported from OpenHuman's
//! `memory_store/trees/`.
//!
//! Three tables (DDL lives in the shared chunk schema, owned by
//! [`crate::memory::chunks`]):
//! - `mem_tree_trees`      — one row per tree (kind, scope, root, max_level)
//! - `mem_tree_summaries`  — one row per sealed summary node (immutable)
//! - `mem_tree_buffers`    — one row per unsealed frontier `(tree_id, level)`
//!
//! Plus the `mem_tree_summary_embeddings` per-model sidecar and the
//! `mem_tree_entity_hotness` side-table. All timestamps are epoch-milliseconds,
//! sharing the convention with `mem_tree_chunks`. Writes serialise through the
//! shared chunk connection ([`crate::memory::chunks::with_connection`]) so this
//! module never opens a second database or schema.

mod buffers;
mod common;
pub mod hotness;
mod summaries;
mod trees;
pub mod types;

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;

// ── Tree rows ───────────────────────────────────────────────────────────────
pub use trees::{
    archive_tree, delete_tree_cascade_tx, get_tree, get_tree_by_scope, get_tree_by_scope_conn,
    get_trees_batch, insert_tree, insert_tree_conn, list_trees_by_kind, refresh_last_sealed_tx,
    update_tree_after_seal_tx, TreeCascadeDeletion,
};

// ── Summary rows + embeddings ───────────────────────────────────────────────
pub use summaries::{
    count_summaries, get_summaries_batch, get_summary, get_summary_embedding,
    get_summary_embedding_for_signature, get_summary_embeddings_batch,
    get_summary_embeddings_for_signature_batch, insert_staged_summary_tx, insert_summary_tx,
    list_children_of_summary, list_summaries_at_level, list_summaries_in_window,
    set_summary_embedding, set_summary_embedding_for_signature,
};

// ── Buffers ─────────────────────────────────────────────────────────────────
pub use buffers::{
    clear_buffer_tx, get_buffer, get_buffer_conn, list_stale_buffers, upsert_buffer_tx,
};

// ── Type + constant re-exports ──────────────────────────────────────────────
pub use types::{
    Buffer, EntityIndexStats, HotnessCounters, SummaryNode, Tree, TreeKind, TreeStatus,
    DEFAULT_FLUSH_AGE_SECS, INPUT_TOKEN_BUDGET, OUTPUT_TOKEN_BUDGET, SUMMARY_FANOUT,
    TOPIC_ARCHIVE_THRESHOLD, TOPIC_CREATION_THRESHOLD, TOPIC_RECHECK_EVERY,
};
