//! On-demand ingest orchestration.
//!
//! Runs the full ingest path for a source-scoped payload supplied by the host:
//!
//! ```text
//! canonicalize -> write raw markdown -> chunk -> score/extract
//!   -> persist chunk metadata -> enqueue tree jobs (-> append/seal in worker)
//! ```
//!
//! TinyCortex does not own live memory sync, so this module assumes the host
//! already produced a `ChatBatch` / `EmailThread` / `DocumentInput`. The buffer
//! append and summary seal happen in the async extract worker driven off the
//! [`TreeJobSink`]; this hot path stops at enqueue.

use anyhow::{anyhow, bail, Result};
use chrono::Utc;

use crate::memory::chunks::{
    self, chunk_markdown, claim_source_ingest_tx, get_chunk_lifecycle_status_tx,
    is_source_ingested, set_chunk_lifecycle_status_tx, set_chunk_raw_refs_tx,
    upsert_staged_chunks_tx, with_connection, ChunkerInput, ChunkerOptions, SourceKind,
    CHUNK_STATUS_PENDING_EXTRACTION,
};
use crate::memory::config::MemoryConfig;
use crate::memory::score::{persist_score_tx, score_chunks_fast, ScoringConfig};
use crate::memory::store::content;

use super::canonicalize::chat::{self, ChatBatch};
use super::canonicalize::document::{self, DocumentInput};
use super::canonicalize::email::{self, EmailThread};
use super::canonicalize::CanonicalisedSource;
use super::types::{IngestOptions, IngestSummary, TreeJobSink};

/// Run the full ingest path for an already-canonicalised source.
///
/// For documents an authoritative source-level gate is claimed transactionally
/// before any chunk is persisted; chat and email have no such gate (their
/// `source_id` is a stream identifier under which many batches accumulate) and
/// rely on deterministic chunk ids for replay idempotency.
///
/// # Atomicity (see `docs/spec/audit/04-queue-ingest.md`)
///
/// - The document gate is claimed in the same SQLite transaction as chunk,
///   score, lifecycle, raw-reference, and extract-job persistence. Scoring and
///   content staging happen first, so no await or filesystem operation splits
///   the authoritative database commit.
/// - Chunk upsert, score persistence, lifecycle transition, raw pointers, and
///   extract-job enqueue commit in one SQLite transaction. The prior lifecycle
///   is read inside that transaction, preventing re-ingest from resetting a
///   chunk a worker has already admitted (QI-12).
/// - Chat/email logical-unit ids derive from complete message content rather
///   than batch-local sequence, so overlapping deliveries reuse persisted rows
///   and active queue dedupe keys.
pub async fn ingest_canonical(
    config: &MemoryConfig,
    source_id: &str,
    canonical: CanonicalisedSource,
    sink: &dyn TreeJobSink,
    scoring: &ScoringConfig,
    opts: &IngestOptions,
) -> Result<IngestSummary> {
    let source_kind = canonical.metadata.source_kind;

    // 1. Chunk the canonical markdown.
    let input = ChunkerInput {
        source_kind,
        source_id: source_id.to_string(),
        markdown: canonical.markdown,
        metadata: canonical.metadata,
    };
    let chunks = chunk_markdown(&input, &ChunkerOptions::default());
    if chunks.is_empty() {
        return Ok(IngestSummary::empty(source_id));
    }

    persist_score_enqueue(config, source_id, source_kind, chunks, sink, scoring, opts).await
}

/// Persist chunk bodies + rows, fast-score, and enqueue extract jobs.
///
async fn persist_score_enqueue(
    config: &MemoryConfig,
    source_id: &str,
    source_kind: SourceKind,
    chunks: Vec<crate::memory::chunks::Chunk>,
    sink: &dyn TreeJobSink,
    scoring: &ScoringConfig,
    opts: &IngestOptions,
) -> Result<IngestSummary> {
    // 3. Write each chunk body to the content store (atomic write + sha256).
    let content_root = chunks::content_root(config);
    let staged = content::stage_chunks(&content_root, &chunks)
        .map_err(|e| anyhow!("stage_chunks failed: {e}"))?;

    // 4. Cheap fast-score (no LLM on the ingest hot path).
    let scores = score_chunks_fast(&chunks, scoring).await?;
    if scores.len() != chunks.len() {
        bail!(
            "scorer length mismatch: chunks={} scores={}",
            chunks.len(),
            scores.len()
        );
    }

    // 5. Persist the whole database tail atomically. Snapshot lifecycle before
    // the upsert while holding this transaction; the worker cannot advance a
    // row between the check and its guarded re-schedule.
    let persisted = with_connection(config, |connection| {
        let transaction = connection.unchecked_transaction()?;
        if source_kind == SourceKind::Document {
            let gate_key = match opts.gate_version_ms {
                Some(version) => format!("{source_id}@{version}"),
                None => source_id.to_string(),
            };
            if !claim_source_ingest_tx(
                &transaction,
                source_kind,
                &gate_key,
                Utc::now().timestamp_millis(),
            )? {
                return Ok(None);
            }
        }
        let prior = chunks
            .iter()
            .map(|chunk| get_chunk_lifecycle_status_tx(&transaction, &chunk.id))
            .collect::<Result<Vec<_>>>()?;
        let chunks_written = upsert_staged_chunks_tx(&transaction, &staged)?;

        if let Some(refs) = opts.raw_refs.as_ref() {
            for chunk in &chunks {
                set_chunk_raw_refs_tx(&transaction, &chunk.id, refs)?;
            }
        }

        let mut jobs = 0usize;
        for ((chunk, result), pre) in chunks.iter().zip(scores.iter()).zip(prior.iter()) {
            let needs_processing =
                matches!(pre.as_deref(), None | Some(CHUNK_STATUS_PENDING_EXTRACTION));
            if !needs_processing {
                continue;
            }
            persist_score_tx(
                &transaction,
                result,
                chunk.metadata.timestamp.timestamp_millis(),
                None,
            )?;
            set_chunk_lifecycle_status_tx(
                &transaction,
                &chunk.id,
                CHUNK_STATUS_PENDING_EXTRACTION,
            )?;
            jobs += usize::from(sink.enqueue_extract_tx(&transaction, &chunk.id)?);
        }
        transaction.commit()?;
        Ok(Some((chunks_written, jobs)))
    })?;

    let Some((chunks_written, extract_jobs_enqueued)) = persisted else {
        return Ok(IngestSummary::already_ingested(source_id));
    };

    let chunks_dropped = scores.iter().filter(|result| !result.kept).count();
    let chunk_ids = chunks.iter().map(|chunk| chunk.id.clone()).collect();

    Ok(IngestSummary {
        source_id: source_id.to_string(),
        chunks_written,
        chunks_dropped,
        chunk_ids,
        extract_jobs_enqueued,
        already_ingested: false,
    })
}

/// Ingest a batch of chat messages. Returns a no-op summary on an empty batch.
#[allow(clippy::too_many_arguments)]
pub async fn ingest_chat(
    config: &MemoryConfig,
    source_id: &str,
    owner: &str,
    tags: Vec<String>,
    batch: ChatBatch,
    sink: &dyn TreeJobSink,
    scoring: &ScoringConfig,
) -> Result<IngestSummary> {
    let canonical =
        match chat::canonicalise(source_id, owner, &tags, batch).map_err(anyhow::Error::msg)? {
            Some(c) => c,
            None => return Ok(IngestSummary::empty(source_id)),
        };
    ingest_canonical(
        config,
        source_id,
        canonical,
        sink,
        scoring,
        &IngestOptions::default(),
    )
    .await
}

/// Ingest an email thread. Returns a no-op summary on an empty thread.
#[allow(clippy::too_many_arguments)]
pub async fn ingest_email(
    config: &MemoryConfig,
    source_id: &str,
    owner: &str,
    tags: Vec<String>,
    thread: EmailThread,
    sink: &dyn TreeJobSink,
    scoring: &ScoringConfig,
) -> Result<IngestSummary> {
    let canonical =
        match email::canonicalise(source_id, owner, &tags, thread).map_err(anyhow::Error::msg)? {
            Some(c) => c,
            None => return Ok(IngestSummary::empty(source_id)),
        };
    ingest_canonical(
        config,
        source_id,
        canonical,
        sink,
        scoring,
        &IngestOptions::default(),
    )
    .await
}

/// Ingest an email thread whose chunk bodies are backed by raw-archive files.
/// The `raw_refs` are attached to every persisted chunk.
#[allow(clippy::too_many_arguments)]
pub async fn ingest_email_with_raw_refs(
    config: &MemoryConfig,
    source_id: &str,
    owner: &str,
    tags: Vec<String>,
    thread: EmailThread,
    raw_refs: Vec<crate::memory::chunks::RawRef>,
    sink: &dyn TreeJobSink,
    scoring: &ScoringConfig,
) -> Result<IngestSummary> {
    let canonical =
        match email::canonicalise(source_id, owner, &tags, thread).map_err(anyhow::Error::msg)? {
            Some(c) => c,
            None => return Ok(IngestSummary::empty(source_id)),
        };
    let opts = IngestOptions {
        gate_version_ms: None,
        raw_refs: Some(raw_refs),
    };
    ingest_canonical(config, source_id, canonical, sink, scoring, &opts).await
}

/// Ingest a single document. Returns a no-op summary on empty input and an
/// already-ingested summary when the document source was previously ingested.
#[allow(clippy::too_many_arguments)]
pub async fn ingest_document(
    config: &MemoryConfig,
    source_id: &str,
    owner: &str,
    tags: Vec<String>,
    doc: DocumentInput,
    sink: &dyn TreeJobSink,
    scoring: &ScoringConfig,
) -> Result<IngestSummary> {
    ingest_document_versioned(
        config, source_id, owner, tags, doc, None, None, sink, scoring,
    )
    .await
}

/// Like [`ingest_document`] but with an explicit `path_scope` (groups multiple
/// items under one content directory while keeping `source_id` as the dedup key).
#[allow(clippy::too_many_arguments)]
pub async fn ingest_document_with_scope(
    config: &MemoryConfig,
    source_id: &str,
    owner: &str,
    tags: Vec<String>,
    doc: DocumentInput,
    path_scope: Option<String>,
    sink: &dyn TreeJobSink,
    scoring: &ScoringConfig,
) -> Result<IngestSummary> {
    ingest_document_versioned(
        config, source_id, owner, tags, doc, path_scope, None, sink, scoring,
    )
    .await
}

/// Version-aware document ingest. When `version_ms` is `Some`, the source gate
/// is keyed by `{source_id}@{version_ms}`, so a later revision of the same
/// document is admitted non-destructively alongside the prior version.
#[allow(clippy::too_many_arguments)]
pub async fn ingest_document_versioned(
    config: &MemoryConfig,
    source_id: &str,
    owner: &str,
    tags: Vec<String>,
    doc: DocumentInput,
    path_scope: Option<String>,
    version_ms: Option<i64>,
    sink: &dyn TreeJobSink,
    scoring: &ScoringConfig,
) -> Result<IngestSummary> {
    // Best-effort pre-canonicalisation gate; the transactional claim inside
    // [`ingest_canonical`] is authoritative.
    let gate_key = match version_ms {
        Some(v) => format!("{source_id}@{v}"),
        None => source_id.to_string(),
    };
    if is_source_ingested(config, SourceKind::Document, &gate_key)? {
        return Ok(IngestSummary::already_ingested(source_id));
    }

    let canonical = match document::canonicalise(source_id, owner, &tags, doc, path_scope)
        .map_err(anyhow::Error::msg)?
    {
        Some(c) => c,
        None => return Ok(IngestSummary::empty(source_id)),
    };
    let opts = IngestOptions {
        gate_version_ms: version_ms,
        raw_refs: None,
    };
    ingest_canonical(config, source_id, canonical, sink, scoring, &opts).await
}

#[cfg(test)]
#[path = "pipeline_tests.rs"]
mod tests;
