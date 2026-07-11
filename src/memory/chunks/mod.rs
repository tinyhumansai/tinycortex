//! Chunks — the unit of memory-store persistence.
//!
//! One module for the full chunk lifecycle, ported faithfully from
//! OpenHuman's `memory_store/chunks`:
//!
//! - [`types`]    — [`Chunk`], [`Metadata`], [`SourceKind`], [`DataSource`],
//!                  [`SourceRef`], [`StagedChunk`] and the deterministic
//!                  [`chunk_id`] / token-estimate functions. The persisted
//!                  shape.
//! - [`produce`]  — source-kind-dispatch chunker (chat / email / document).
//!                  Splits canonical Markdown into bounded chunks with stable
//!                  per-source sequence numbers.
//! - [`semantic`] — heading- and paragraph-aware chunker used to split large
//!                  documents into LLM-context-sized pieces while preserving
//!                  heading context. Exported as [`chunk_semantic`].
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

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use crate::memory::config::MemoryConfig;

mod schema;

#[path = "connection.rs"]
mod connection;
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

pub use connection::with_connection;
pub use embeddings::{
    clear_chunk_reembed_skipped, clear_reembed_skipped_for_signature, get_chunk_embedding,
    get_chunk_embedding_for_signature, get_chunk_embeddings_batch,
    get_chunk_embeddings_for_signature_batch, mark_chunk_reembed_skipped, set_chunk_embedding,
    set_chunk_embedding_for_signature, tree_active_signature,
};
pub use raw_refs::{
    get_chunk_content_path, get_chunk_content_pointers, get_chunk_raw_refs,
    get_summary_content_pointers, list_chunk_raw_ref_paths_with_prefix,
    list_summaries_with_content_path, set_chunk_raw_refs, set_chunk_raw_refs_tx, RawRef,
};
pub use store::{
    claim_source_ingest_tx, count_chunks, count_chunks_by_lifecycle_status,
    count_raw_paths_ingested_with_prefix, delete_source_ingest, extraction_coverage,
    filter_raw_paths_not_ingested, get_chunk, get_chunk_lifecycle_status, get_chunks_batch,
    is_source_ingested, list_chunks, mark_raw_paths_ingested, set_chunk_lifecycle_status,
    upsert_chunks, ListChunksQuery, CHUNK_STATUS_ADMITTED, CHUNK_STATUS_BUFFERED,
    CHUNK_STATUS_DROPPED, CHUNK_STATUS_PENDING_EXTRACTION, CHUNK_STATUS_SEALED, RAW_FILE_GATE_KIND,
};
pub use store_delete::{
    delete_chunks_by_owner, delete_chunks_by_source, delete_chunks_by_source_prefix,
};

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
    config.workspace.join(DB_DIR).join("content")
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

/// Tag marking a chunk as belonging to a configurable "memory source".
const MEMORY_SOURCE_TAG: &str = "memory_sources";

/// Extract the memory-source id from a composite `mem_src:<id>:<item>` source
/// id, or `None` when the id is not in that format. Mirrors OpenHuman's
/// `memory::sync::extract_mem_src_id`.
fn extract_mem_src_id(composite_source_id: &str) -> Option<&str> {
    let rest = composite_source_id.strip_prefix("mem_src:")?;
    let colon_pos = rest.find(':')?;
    let source_id = &rest[..colon_pos];
    if colon_pos + 1 >= rest.len() {
        return None;
    }
    Some(source_id)
}

/// Whether a chunk is allowed under a per-profile memory-source allowlist.
/// Non-memory-source chunks always pass; memory-source chunks pass only when
/// their (possibly composite) source id resolves into `set`.
pub(crate) fn chunk_source_allowed_in(
    set: &HashSet<String>,
    tags: &[String],
    source_id: &str,
) -> bool {
    let is_memory_source = tags.iter().any(|t| t == MEMORY_SOURCE_TAG);
    if !is_memory_source {
        return true;
    }
    if set.contains(source_id) {
        return true;
    }
    extract_mem_src_id(source_id).is_some_and(|id| set.contains(id))
}
