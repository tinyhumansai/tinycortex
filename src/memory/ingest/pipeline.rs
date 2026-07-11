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
    self, chunk_markdown, claim_source_ingest_tx, delete_source_ingest, get_chunk_lifecycle_status,
    is_source_ingested, set_chunk_lifecycle_status, set_chunk_raw_refs, upsert_chunks,
    with_connection, ChunkerInput, ChunkerOptions, SourceKind, CHUNK_STATUS_PENDING_EXTRACTION,
};
use crate::memory::config::MemoryConfig;
use crate::memory::score::{persist_score, score_chunks_fast, ScoringConfig};
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

    // 2. Authoritative source gate (documents only). Claimed before any write
    //    so two concurrent ingests of the same document can't both proceed.
    //
    //    The gate row is committed in its own transaction (staging/scoring is
    //    async and can't hold a SQLite transaction across `.await`). To keep the
    //    gate honest we treat it as a *reservation*: if any later stage fails we
    //    release it (see the compensation below) so the document is never left
    //    permanently marked ingested with zero persisted chunks — a retry can
    //    then re-claim and finish the job.
    let gate_key: Option<String> = if source_kind == SourceKind::Document {
        let gate_key = match opts.gate_version_ms {
            Some(v) => format!("{source_id}@{v}"),
            None => source_id.to_string(),
        };
        let claimed = with_connection(config, |conn| {
            let tx = conn.unchecked_transaction()?;
            let claimed =
                claim_source_ingest_tx(&tx, source_kind, &gate_key, Utc::now().timestamp_millis())?;
            tx.commit()?;
            Ok(claimed)
        })?;
        if !claimed {
            return Ok(IngestSummary::already_ingested(source_id));
        }
        Some(gate_key)
    } else {
        None
    };

    // Run the remaining persist/score/enqueue stages. On any failure after the
    // gate was claimed, release the reservation so a retry is not short-circuited.
    let result = persist_score_enqueue(config, source_id, chunks, sink, scoring, opts).await;
    if result.is_err() {
        if let Some(key) = gate_key.as_deref() {
            if let Err(cleanup_err) = delete_source_ingest(config, source_kind, key) {
                // Best-effort: the original error is the one worth surfacing, but
                // a failed release would leave the source wrongly gated, so make
                // the operator aware of it.
                eprintln!(
                    "ingest: failed to release source gate {key} after ingest error: {cleanup_err}"
                );
            }
        }
    }
    result
}

/// Persist chunk bodies + rows, fast-score, and enqueue extract jobs.
///
/// Split out from [`ingest_canonical`] so the source-gate reservation can wrap
/// exactly this fallible tail and compensate (release the gate) on any error.
async fn persist_score_enqueue(
    config: &MemoryConfig,
    source_id: &str,
    chunks: Vec<crate::memory::chunks::Chunk>,
    sink: &dyn TreeJobSink,
    scoring: &ScoringConfig,
    opts: &IngestOptions,
) -> Result<IngestSummary> {
    // 3. Write each chunk body to the content store (atomic write + sha256).
    let content_root = chunks::content_root(config);
    content::stage_chunks(&content_root, &chunks)
        .map_err(|e| anyhow!("stage_chunks failed: {e}"))?;

    // 4. Snapshot each chunk's CURRENT lifecycle BEFORE the upsert. A chunk that
    //    already progressed past `pending_extraction` on a prior ingest must not
    //    be re-scheduled, or already-buffered/sealed content would flow through
    //    the tree twice.
    let mut prior: Vec<Option<String>> = Vec::with_capacity(chunks.len());
    for chunk in &chunks {
        prior.push(get_chunk_lifecycle_status(config, &chunk.id)?);
    }

    // 5. Persist chunk rows (idempotent on deterministic chunk id).
    let chunks_written = upsert_chunks(config, &chunks)?;

    // 5b. Raw-archive-backed bodies: attach refs so a worker can resolve them.
    if let Some(refs) = opts.raw_refs.as_ref() {
        for chunk in &chunks {
            set_chunk_raw_refs(config, &chunk.id, refs)?;
        }
    }

    // 6. Cheap fast-score (no LLM on the ingest hot path).
    let scores = score_chunks_fast(&chunks, scoring).await?;
    if scores.len() != chunks.len() {
        bail!(
            "scorer length mismatch: chunks={} scores={}",
            chunks.len(),
            scores.len()
        );
    }

    // 7. Persist scores, mark newly-scheduled chunks pending, and enqueue an
    //    extract job per scheduled chunk. The fast-score `kept` flag only feeds
    //    the dropped count for reporting — final admission happens in the worker.
    let mut chunks_dropped = 0usize;
    let mut extract_jobs_enqueued = 0usize;
    let mut chunk_ids = Vec::with_capacity(chunks.len());
    for ((chunk, result), pre) in chunks.iter().zip(scores.iter()).zip(prior.iter()) {
        chunk_ids.push(chunk.id.clone());
        if !result.kept {
            chunks_dropped += 1;
        }

        let needs_processing =
            matches!(pre.as_deref(), None | Some(CHUNK_STATUS_PENDING_EXTRACTION));
        if !needs_processing {
            continue;
        }

        let ts_ms = chunk.metadata.timestamp.timestamp_millis();
        persist_score(config, result, ts_ms, None)?;
        set_chunk_lifecycle_status(config, &chunk.id, CHUNK_STATUS_PENDING_EXTRACTION)?;
        sink.enqueue_extract(&chunk.id)?;
        extract_jobs_enqueued += 1;
    }

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
