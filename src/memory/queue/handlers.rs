//! Per-`JobKind` dispatch for the worker pool.
//!
//! The queue keeps its own control flow in this module — parsing payloads,
//! enqueuing follow-up jobs, deciding `Done` vs `Defer` — and pushes only the
//! genuinely external heavy work (LLM scoring/extraction, buffer pushes,
//! sealing, embedding) behind the [`QueueDelegates`] trait. The OpenHuman
//! handlers called directly into `memory_tree` / `memory_score` /
//! `memory_store`, but those operations are exposed only `pub(crate)` to their
//! own modules (and some — the write-path embedder, the re-embed worklist
//! probe, the on-disk body reader — are not part of this crate's ported
//! surface at all). The trait is that seam: a host that owns visibility into
//! those modules supplies a real implementation; tests supply a deterministic
//! one (see `handlers_tests.rs`).
//!
//! What stays in-crate (faithful to OpenHuman's pipeline shape):
//! - `extract_chunk` admits → enqueues `append_buffer`, then arms the re-embed
//!   backfill once anything was admitted.
//! - `append_buffer` pushes a leaf → enqueues `seal` when the gate is met.
//! - `seal` seals one level → enqueues the parent `seal` when cascading.
//! - `flush_stale` enqueues a force-`seal` per stale buffer.
//! - `reembed_backfill` maps a bounded batch to `Defer` (more pending) or
//!   `Done` (covered / no provider / stale signature), toggling the
//!   process-global backfill flag.

use anyhow::{Context, Result};
use async_trait::async_trait;

use crate::memory::config::MemoryConfig;
use crate::memory::queue::ops::set_backfill_in_progress;
use crate::memory::queue::store;
use crate::memory::queue::types::{
    AppendBufferPayload, AppendTarget, ExtractChunkPayload, FlushStalePayload, Job, JobKind,
    JobOutcome, NewJob, NodeRef, ReembedBackfillPayload, SealDocumentPayload, SealPayload,
};

/// Default age for an L0 `flush_stale` when the payload doesn't override it.
/// One hour means low-volume sources get summaries within a working session.
pub const L0_DEFAULT_FLUSH_AGE_SECS: i64 = 60 * 60;

/// Delay before a deferred re-embed chain revisits its own row.
pub const REEMBED_BACKFILL_REVISIT_MS: i64 = 750;

/// Outcome of the external `extract_chunk` step (LLM scoring + admission +
/// score/lifecycle persistence). The queue uses it to decide the follow-up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtractDecision {
    /// Whether the chunk was admitted into the tree pipeline.
    pub kept: bool,
    /// Whether the chunk's source uses the per-document rollup path (Notion),
    /// which builds its tree via `SealDocument` rather than the flat L0 buffer.
    pub uses_document_subtree: bool,
    /// The source-tree scope to append this leaf under (GitHub-aware / path
    /// scope). Only consulted when `kept && !uses_document_subtree`.
    pub tree_scope: String,
}

/// Outcome of the external `append_buffer` step (buffer push + gate check).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppendDecision {
    /// The physical tree id the leaf landed in.
    pub tree_id: String,
    /// Whether the L0 buffer crossed its seal gate during this push.
    pub should_seal: bool,
}

/// A stale buffer that `flush_stale` should force-seal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StaleBuffer {
    /// Physical tree id of the stale buffer to force-seal.
    pub tree_id: String,
    /// Buffer level (0 = leaf L0; higher = summary tiers).
    pub level: u32,
}

/// Outcome of one external re-embed batch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReembedProgress {
    /// A batch was embedded; `more_pending` drives `Defer` vs `Done`.
    Wrote {
        /// Whether more rows remain to re-embed (drives `Defer` vs `Done`).
        more_pending: bool,
    },
    /// The signature space is fully covered — finish the chain.
    Covered,
    /// No usable embeddings provider — skip (rows stay re-embeddable).
    NoProvider,
    /// The active signature changed since this chain started — finish.
    StaleSignature,
}

/// The external, heavy-work seam for the queue handlers. Every method is the
/// part the queue cannot do itself (it needs `memory_tree` / `memory_score` /
/// `memory_store` internals). Implementations must be deterministic enough for
/// tests; production impls wire the real subsystems.
#[async_trait]
pub trait QueueDelegates: Send + Sync {
    /// Score + admit one chunk and persist its score/lifecycle. `Ok(None)` when
    /// the chunk row is missing (a no-op `Done`).
    async fn extract_chunk(
        &self,
        config: &MemoryConfig,
        chunk_id: &str,
    ) -> Result<Option<ExtractDecision>>;

    /// Push a node into its target buffer. `Ok(None)` when the node or target
    /// tree is missing (a no-op `Done`).
    async fn append_node(
        &self,
        config: &MemoryConfig,
        node: &NodeRef,
        target: &AppendTarget,
    ) -> Result<Option<AppendDecision>>;

    /// Seal one buffer level. Returns the parent-level `SealPayload` to enqueue
    /// when the seal cascades, else `None`.
    async fn seal_level(
        &self,
        config: &MemoryConfig,
        payload: &SealPayload,
    ) -> Result<Option<SealPayload>>;

    /// List buffers older than `max_age_secs` that should be force-sealed.
    async fn list_stale_buffers(
        &self,
        config: &MemoryConfig,
        max_age_secs: i64,
    ) -> Result<Vec<StaleBuffer>>;

    /// Build + merge one document version's subtree.
    async fn seal_document(
        &self,
        config: &MemoryConfig,
        payload: &SealDocumentPayload,
    ) -> Result<()>;

    /// Embed one bounded re-embed batch at `signature`.
    async fn reembed_batch(
        &self,
        config: &MemoryConfig,
        signature: &str,
    ) -> Result<ReembedProgress>;

    /// The active embedding signature (for the re-embed switch-path trigger).
    fn active_signature(&self, config: &MemoryConfig) -> String;

    /// Whether any chunk/summary lacks a vector at `signature`.
    fn has_uncovered_reembed_work(&self, config: &MemoryConfig, signature: &str) -> Result<bool>;
}

/// Dispatch a claimed job to the matching per-kind handler.
pub async fn handle_job(
    config: &MemoryConfig,
    job: &Job,
    delegates: &dyn QueueDelegates,
) -> Result<JobOutcome> {
    match job.kind {
        JobKind::ExtractChunk => handle_extract(config, job, delegates).await,
        JobKind::AppendBuffer => handle_append_buffer(config, job, delegates).await,
        JobKind::Seal => handle_seal(config, job, delegates).await,
        JobKind::FlushStale => handle_flush_stale(config, job, delegates).await,
        JobKind::ReembedBackfill => handle_reembed_backfill(config, job, delegates).await,
        JobKind::SealDocument => handle_seal_document(config, job, delegates).await,
    }
}

async fn handle_extract(
    config: &MemoryConfig,
    job: &Job,
    delegates: &dyn QueueDelegates,
) -> Result<JobOutcome> {
    let payload: ExtractChunkPayload =
        serde_json::from_str(&job.payload_json).context("parse ExtractChunk payload")?;
    let Some(decision) = delegates.extract_chunk(config, &payload.chunk_id).await? else {
        // Chunk row vanished between enqueue and claim — nothing to do.
        return Ok(JobOutcome::Done);
    };

    // Admitted, flat-buffer source: enqueue the append-buffer follow-up. The
    // per-document-versioned sources (Notion) skip the flat L0 buffer — their
    // tree is built by a SealDocument job enqueued at ingest.
    if decision.kept && !decision.uses_document_subtree {
        let follow_up = NewJob::append_buffer(&AppendBufferPayload {
            node: NodeRef::Leaf {
                chunk_id: payload.chunk_id.clone(),
            },
            target: AppendTarget::Source {
                source_id: decision.tree_scope.clone(),
            },
        })?;
        store::enqueue(config, &follow_up)?;
    }

    // Anything admitted arms the re-embed backfill so the embedding pass starts
    // promptly (extract no longer embeds inline).
    if decision.kept {
        crate::memory::queue::ops::ensure_reembed_backfill(config, delegates)?;
    }

    Ok(JobOutcome::Done)
}

async fn handle_append_buffer(
    config: &MemoryConfig,
    job: &Job,
    delegates: &dyn QueueDelegates,
) -> Result<JobOutcome> {
    let payload: AppendBufferPayload =
        serde_json::from_str(&job.payload_json).context("parse AppendBuffer payload")?;
    let Some(decision) = delegates
        .append_node(config, &payload.node, &payload.target)
        .await?
    else {
        // Missing chunk/summary, or the target topic tree was archived — drop.
        return Ok(JobOutcome::Done);
    };

    if decision.should_seal {
        let seal = SealPayload {
            tree_id: decision.tree_id,
            level: 0,
            force_now_ms: None,
        };
        store::enqueue(config, &NewJob::seal(&seal)?)?;
    }
    Ok(JobOutcome::Done)
}

async fn handle_seal(
    config: &MemoryConfig,
    job: &Job,
    delegates: &dyn QueueDelegates,
) -> Result<JobOutcome> {
    let payload: SealPayload =
        serde_json::from_str(&job.payload_json).context("parse Seal payload")?;
    // Seal exactly one level. A cascading seal returns the parent payload so
    // each level stays its own crash-recovery checkpoint.
    if let Some(parent) = delegates.seal_level(config, &payload).await? {
        store::enqueue(config, &NewJob::seal(&parent)?)?;
    }
    Ok(JobOutcome::Done)
}

async fn handle_flush_stale(
    config: &MemoryConfig,
    job: &Job,
    delegates: &dyn QueueDelegates,
) -> Result<JobOutcome> {
    let payload: FlushStalePayload =
        serde_json::from_str(&job.payload_json).context("parse FlushStale payload")?;
    let age_secs = payload.max_age_secs.unwrap_or(L0_DEFAULT_FLUSH_AGE_SECS);
    let now_ms = chrono::Utc::now().timestamp_millis();
    for buf in delegates.list_stale_buffers(config, age_secs).await? {
        let seal = SealPayload {
            tree_id: buf.tree_id,
            level: buf.level,
            force_now_ms: Some(now_ms),
        };
        store::enqueue(config, &NewJob::seal(&seal)?)?;
    }
    Ok(JobOutcome::Done)
}

async fn handle_seal_document(
    config: &MemoryConfig,
    job: &Job,
    delegates: &dyn QueueDelegates,
) -> Result<JobOutcome> {
    let payload: SealDocumentPayload =
        serde_json::from_str(&job.payload_json).context("parse SealDocument payload")?;
    if payload.chunk_ids.is_empty() {
        // Empty version set — nothing to seal.
        return Ok(JobOutcome::Done);
    }
    delegates.seal_document(config, &payload).await?;
    Ok(JobOutcome::Done)
}

async fn handle_reembed_backfill(
    config: &MemoryConfig,
    job: &Job,
    delegates: &dyn QueueDelegates,
) -> Result<JobOutcome> {
    let payload: ReembedBackfillPayload =
        serde_json::from_str(&job.payload_json).context("parse ReembedBackfill payload")?;

    match delegates.reembed_batch(config, &payload.signature).await? {
        ReembedProgress::Wrote {
            more_pending: true, ..
        } => {
            set_backfill_in_progress(true);
            // More rows may remain — reschedule THIS row (no re-enqueue, so the
            // per-signature dedupe key stays valid).
            Ok(JobOutcome::Defer {
                until_ms: chrono::Utc::now().timestamp_millis() + REEMBED_BACKFILL_REVISIT_MS,
                reason: "re-embed backfill: batch done, more pending".to_string(),
            })
        }
        ReembedProgress::Wrote {
            more_pending: false,
        }
        | ReembedProgress::Covered
        | ReembedProgress::NoProvider
        | ReembedProgress::StaleSignature => {
            set_backfill_in_progress(false);
            Ok(JobOutcome::Done)
        }
    }
}

#[cfg(test)]
#[path = "handlers_tests.rs"]
mod tests;
