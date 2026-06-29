//! Async job queue for memory-tree work.
//!
//! Replaces the synchronous `append_leaf → cascade_seal → LLM summarise` chain
//! on the ingest hot path with a SQLite-backed job queue and a worker driver.
//! The shape is:
//!
//! ```text
//! ingest::persist
//!   └── writes chunk row (lifecycle = pending_extraction)
//!       enqueues `extract_chunk`
//!
//! run_once (driven in a host loop) claims jobs by kind:
//!   extract_chunk   → score/admit → enqueue append_buffer + arm reembed
//!   append_buffer   → push to L0 → enqueue seal if gate met
//!   seal            → seal one level → enqueue parent seal if cascading
//!   flush_stale     → enqueue force-seals for time-stale buffers
//!   reembed_backfill→ embed a bounded batch → Defer until covered
//!   seal_document   → build one document version's subtree
//! ```
//!
//! All persistence lives in the same `chunks.db` as `mem_tree_chunks` (the
//! `mem_tree_jobs` table is owned by the shared chunks schema), so a producer
//! can insert its side-effect and its follow-up job in one transaction — see
//! [`store::enqueue_tx`].
//!
//! ## Differences from OpenHuman
//!
//! - The `tokio` worker pool + wall-clock scheduler are reduced to [`run_once`]
//!   plus plain scheduler functions a host drives, since `tokio` is a dev-only
//!   dependency and the spawn/shutdown/Sentry plumbing is out of this crate's
//!   surface. [`drain_until_idle`] settles the queue deterministically.
//! - The heavy per-kind work (LLM scoring/extraction, buffer push, sealing,
//!   embedding) sits behind the [`QueueDelegates`] trait because those
//!   operations are exposed only `pub(crate)` to `memory::tree` / `memory::score`
//!   / `memory::chunks` (and some are not part of the ported surface). The
//!   queue keeps its own logic — payload parsing, follow-up enqueues, gating,
//!   defer — in-crate.
//! - The typed-failure classifier is the self-contained [`JobFailure`] rather
//!   than the upstream `memory_tree::health::PipelineFailure`.

pub mod gate;
mod handlers;
mod ops;
mod redact;
pub mod scheduler;
pub mod store;
pub mod store_settle;
pub mod testing;
pub mod types;
pub mod worker;

#[cfg(test)]
pub(crate) mod test_support;

pub use gate::{LlmGate, Permit, DEFAULT_LLM_PERMITS};
pub use handlers::{
    handle_job, AppendDecision, ExtractDecision, QueueDelegates, ReembedProgress, StaleBuffer,
};
pub use ops::{backfill_in_progress, ensure_reembed_backfill, set_backfill_in_progress};
pub use redact::scrub_for_log;
pub use store::{
    claim_next, count_by_status, count_failed_unrecoverable, count_total, enqueue, enqueue_tx,
    get_job, is_retired_kind, purge_retired_jobs, DEFAULT_LOCK_DURATION_MS,
};
pub use store_settle::{
    mark_deferred, mark_done, mark_failed, mark_failed_typed, recover_stale_locks,
    release_running_locks, requeue_failed, requeue_transient_failed, retry_all_failed,
};
pub use testing::drain_until_idle;
pub use types::{
    AppendBufferPayload, AppendTarget, ExtractChunkPayload, FlushStalePayload, Job, JobFailure,
    JobKind, JobOutcome, JobStatus, NewJob, NodeRef, ReembedBackfillPayload, SealDocumentPayload,
    SealPayload,
};
pub use worker::{bootstrap, llm_gate, run_once};
