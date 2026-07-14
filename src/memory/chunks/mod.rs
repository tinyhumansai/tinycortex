//! Chunks — the unit of memory-store persistence.
//!
//! One module for the full chunk lifecycle, ported faithfully from
//! OpenHuman's `memory_store/chunks`:
//!
//! - `types` — [`Chunk`], [`Metadata`], [`SourceKind`], [`DataSource`],
//!   [`SourceRef`], [`StagedChunk`] and the deterministic
//!   [`chunk_id`] / token-estimate functions. The persisted shape.
//! - `produce` — source-kind-dispatch chunker (chat / email / document).
//!   Splits canonical Markdown into bounded chunks with stable per-source
//!   sequence numbers.
//! - `semantic` — heading- and paragraph-aware chunker used to split large
//!   documents into LLM-context-sized pieces while preserving heading context.
//!   Exported as [`chunk_semantic`].
//! - `store` / `connection` / `migrations` / `raw_refs` / `embeddings` —
//!   the SQLite-backed chunk store (the `mem_tree_chunks` table plus its
//!   per-model embedding sidecars and source ingest gates).
//!
//! ## Differences from OpenHuman
//!
//! This is the storage-layer slice only. Pieces that hard-depend on the
//! summary-tree, async-queue, and embedding-backend subsystems (not yet
//! ported) are abstracted away: the SQLite *schema* still declares every
//! `mem_tree_*` table (it is pure DDL), but the Rust code here never calls
//! into those modules. The chunk store owns the connection cache and the
//! one-shot SQLite migrations; everything else is left to the modules that
//! own those tables.

use std::path::PathBuf;
use std::time::Duration;

use crate::memory::config::MemoryConfig;

mod schema;

#[path = "connection.rs"]
mod connection;
mod connection_breaker;
#[path = "recovery.rs"]
mod recovery;

#[path = "embeddings.rs"]
mod embeddings;
#[path = "migrations.rs"]
mod migrations;
#[path = "produce.rs"]
mod produce;
#[path = "produce_split.rs"]
mod produce_split;
#[path = "raw_refs.rs"]
mod raw_refs;
#[path = "semantic.rs"]
mod semantic;
#[path = "store.rs"]
mod store;
#[path = "store_delete.rs"]
mod store_delete;
mod store_list;
#[path = "store_sources.rs"]
mod store_sources;
#[path = "types.rs"]
mod types;

#[cfg(test)]
#[path = "store_conn_tests.rs"]
mod store_conn_tests;
#[cfg(test)]
#[path = "store_embed_tests.rs"]
mod store_embed_tests;
#[cfg(test)]
#[path = "store_tests.rs"]
mod store_tests;

// ── Public API surface ───────────────────────────────────────────────────────

pub use produce::{chunk_markdown, ChunkerInput, ChunkerOptions, DEFAULT_CHUNK_MAX_TOKENS};
pub use semantic::{chunk_markdown as chunk_semantic, Chunk as SemanticChunk};
pub use types::{
    approx_token_count, chunk_id, conservative_token_estimate, truncate_to_conservative_tokens,
    Chunk, DataSource, Metadata, SourceKind, SourceRef, StagedChunk,
};

pub use connection::{shared_connection, with_connection};
pub use embeddings::{
    clear_chunk_reembed_skipped, clear_reembed_skipped_for_signature,
    clear_summary_reembed_skipped, embedding_to_blob, get_chunk_embedding,
    get_chunk_embedding_for_signature, get_chunk_embeddings_batch,
    get_chunk_embeddings_for_signature_batch, has_uncovered_reembed_work,
    mark_chunk_reembed_skipped, mark_summary_reembed_skipped, set_chunk_embedding,
    set_chunk_embedding_for_signature, set_chunk_embedding_for_signature_tx,
    set_summary_embedding_for_signature_tx, tree_active_signature, REEMBED_SKIP_KEY_MAX_LEN,
};
pub use raw_refs::{
    get_chunk_content_path, get_chunk_content_pointers, get_chunk_raw_refs,
    get_summary_content_pointers, list_chunk_raw_ref_paths_with_prefix,
    list_summaries_with_content_path, set_chunk_raw_refs, set_chunk_raw_refs_tx, RawRef,
};
pub use recovery::{
    is_io_open_error, is_transient_cold_start, recover_corrupt_db, try_cleanup_stale_files,
};
pub use store::{
    claim_source_ingest_tx, count_chunks, count_chunks_by_lifecycle_status,
    count_raw_paths_ingested_with_prefix, delete_source_ingest, extraction_coverage,
    filter_raw_paths_not_ingested, get_chunk, get_chunk_lifecycle_status, get_chunks_batch,
    is_source_ingested, list_source_ids_with_prefix, mark_raw_paths_ingested,
    set_chunk_lifecycle_status, update_chunk_content_sha256, update_summary_content_sha256,
    upsert_chunks, upsert_chunks_tx, upsert_staged_chunks_tx, CHUNK_STATUS_ADMITTED,
    CHUNK_STATUS_BUFFERED, CHUNK_STATUS_DROPPED, CHUNK_STATUS_PENDING_EXTRACTION,
    CHUNK_STATUS_SEALED, RAW_FILE_GATE_KIND,
};
pub(crate) use store_delete::remove_unreferenced_content_files;
pub use store_delete::{
    delete_chunks_by_owner, delete_chunks_by_source, delete_chunks_by_source_prefix,
    delete_orphaned_source_tree,
};
pub use store_list::{list_chunks, ListChunksQuery};
pub use store_sources::{get_chunk_lifecycle_status_tx, set_chunk_lifecycle_status_tx};

// ── Shared internal constants / helpers ─────────────────────────────────────

/// Sub-directory of the workspace holding the chunk SQLite database.
pub(crate) const DB_DIR: &str = "memory_tree";
/// File name of the chunk SQLite database (under [`DB_DIR`]).
pub(crate) const DB_FILE: &str = "chunks.db";
/// Busy-handler timeout for the chunk DB. 15s absorbs transient write-lock
/// contention inside rusqlite instead of surfacing `SQLITE_BUSY` to callers.
pub(crate) const SQLITE_BUSY_TIMEOUT: Duration = Duration::from_secs(15);

/// `PRAGMA user_version` value once the one-shot legacy→sidecar embedding
/// migration has run. `0` (fresh/legacy DB) triggers the copy on next open.
pub(crate) const TREE_EMBEDDING_MIGRATION_VERSION: i64 = 1;
/// `PRAGMA user_version` value once the global/topic-tree purge has run.
pub(crate) const GLOBAL_TOPIC_PURGE_MIGRATION_VERSION: i64 = 2;

/// Canonical chunk DB path for `config`'s workspace.
pub(crate) fn db_path_for(config: &MemoryConfig) -> PathBuf {
    config.workspace.join(DB_DIR).join(DB_FILE)
}

/// Root directory for on-disk chunk/summary content files associated with the
/// chunk DB. Bodies that are too large for the SQLite preview column live here.
pub(crate) fn content_root(config: &MemoryConfig) -> PathBuf {
    config
        .content_root
        .clone()
        .unwrap_or_else(|| config.workspace.join(DB_DIR).join("content"))
}

/// Redact a PII-bearing string for log output by hashing it to 8 hex chars.
/// Stable across runs for the same input (so it stays greppable) but never
/// reveals the original value (source ids and content paths can embed emails).
#[allow(dead_code)]
pub(crate) fn redact(s: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    let d = h.finalize();
    format!("{:08x}", u32::from_be_bytes([d[0], d[1], d[2], d[3]]))
}
