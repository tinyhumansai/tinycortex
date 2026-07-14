//! Types for the on-demand ingest orchestration: the job-sink seam, per-call
//! options, and the result summary.

use anyhow::Result;
use rusqlite::Transaction;

use crate::memory::chunks::RawRef;
use crate::memory::queue::types::{ExtractChunkPayload, NewJob};

/// Sink for the tree jobs the ingest path produces.
///
/// Implementations enqueue against the caller-owned chunk-database transaction
/// so chunk lifecycle and queue visibility commit atomically.
pub trait TreeJobSink: Send + Sync {
    /// Enqueue an `extract_chunk` job for `chunk_id` inside `tx`. Returns true
    /// when a new job was created and false for a deliberate no-op or dedupe.
    fn enqueue_extract_tx(&self, tx: &Transaction<'_>, chunk_id: &str) -> Result<bool>;
}

/// A no-op [`TreeJobSink`] that drops every job. Useful when a caller only wants
/// chunks persisted and scored without driving the tree.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullJobSink;

impl TreeJobSink for NullJobSink {
    fn enqueue_extract_tx(&self, _tx: &Transaction<'_>, _chunk_id: &str) -> Result<bool> {
        Ok(false)
    }
}

/// Production sink backed by the durable SQLite queue.
#[derive(Debug, Default, Clone, Copy)]
pub struct QueueJobSink;

impl TreeJobSink for QueueJobSink {
    fn enqueue_extract_tx(&self, tx: &Transaction<'_>, chunk_id: &str) -> Result<bool> {
        let job = NewJob::extract_chunk(&ExtractChunkPayload {
            chunk_id: chunk_id.to_string(),
        })?;
        Ok(crate::memory::queue::store::enqueue_tx(tx, &job)?.is_some())
    }
}

/// Per-call ingest knobs.
#[derive(Debug, Default, Clone)]
pub struct IngestOptions {
    /// When set, the document source gate is keyed by `{source_id}@{version_ms}`
    /// so a later revision of the same document is admitted non-destructively.
    /// Ignored for chat/email (which have no source-level gate).
    pub gate_version_ms: Option<i64>,
    /// When set, every persisted chunk is annotated with these raw-archive refs
    /// so a worker can resolve the chunk body from a verbatim source file.
    pub raw_refs: Option<Vec<RawRef>>,
}

/// Outcome of one ingest call.
///
/// `extract_jobs_enqueued` counts the leaves handed to the [`TreeJobSink`] for
/// downstream tree append; the actual buffer append and summary seal happen in
/// the (separately ported) async worker, so this summary reports work *pending*,
/// not completed.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct IngestSummary {
    /// Logical source id this call ingested.
    pub source_id: String,
    /// Number of chunks persisted to the chunk store.
    pub chunks_written: usize,
    /// Number of chunks the cheap fast-score path would drop. Final admission
    /// still happens later in the extract worker.
    pub chunks_dropped: usize,
    /// Ids of all chunks produced by this call.
    pub chunk_ids: Vec<String>,
    /// Number of `extract_chunk` jobs enqueued (leaves pending tree append).
    pub extract_jobs_enqueued: usize,
    /// True when this ingest was a no-op because `(source_kind, source_id)` was
    /// already ingested. Documents are append-only — the summariser tree must
    /// not see the same source twice.
    pub already_ingested: bool,
}

impl IngestSummary {
    /// A no-op summary for empty/no-chunk payloads.
    pub(super) fn empty(source_id: &str) -> Self {
        Self {
            source_id: source_id.to_string(),
            chunks_written: 0,
            chunks_dropped: 0,
            chunk_ids: Vec::new(),
            extract_jobs_enqueued: 0,
            already_ingested: false,
        }
    }

    /// A summary marking the source as already ingested (gate lost / dup).
    pub(super) fn already_ingested(source_id: &str) -> Self {
        Self {
            source_id: source_id.to_string(),
            chunks_written: 0,
            chunks_dropped: 0,
            chunk_ids: Vec::new(),
            extract_jobs_enqueued: 0,
            already_ingested: true,
        }
    }
}
