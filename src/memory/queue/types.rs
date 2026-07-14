//! Job types for the async memory-tree pipeline.
//!
//! Each `Job` row in `mem_tree_jobs` stores its discriminator as a string
//! `kind` plus a JSON-encoded `payload`. The strongly-typed payload structs
//! below own (de)serialisation; handlers parse the payload by branching on
//! [`JobKind`] and calling the matching `from_payload_json`.
//!
//! Ported from OpenHuman's `memory_queue::types`. The one substitution: the
//! upstream typed-failure classifier lived in `memory_tree::health`
//! (`PipelineFailure`), which is not part of this crate's ported surface, so a
//! small self-contained [`JobFailure`] replaces it here. It preserves the same
//! contract the queue cares about: a machine-readable `code`, a `class`
//! (`transient` / `unrecoverable`), and [`JobFailure::is_unrecoverable`].

use anyhow::{anyhow, Result};

pub use super::payloads::{
    AppendBufferPayload, AppendTarget, ExtractChunkPayload, FlushStalePayload, NodeRef,
    ReembedBackfillPayload, SealDocumentPayload, SealPayload,
};

/// Discriminator persisted in `mem_tree_jobs.kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum JobKind {
    /// Run LLM entity extraction over a single chunk and decide admission.
    ExtractChunk,
    /// Push an admitted chunk into a tree's L0 buffer.
    AppendBuffer,
    /// Seal exactly one buffer level; cascades enqueue a follow-up.
    Seal,
    /// Walk stale buffers and enqueue `Seal` jobs for any over the age cap.
    FlushStale,
    /// Re-embed a bounded batch of chunks/summaries that lack a vector at the
    /// active embedding signature (post model-switch, or the dim-mismatch
    /// slice), then self-continue until none remain.
    ReembedBackfill,
    /// Build one document version's per-doc subtree and merge its doc-root into
    /// the connection tree. Replaces the per-chunk extract→append_buffer tree
    /// path for document sources that opt into per-document rollup + versioning.
    SealDocument,
}

impl JobKind {
    /// Snake-case wire string written to `mem_tree_jobs.kind`.
    pub fn as_str(&self) -> &'static str {
        match self {
            JobKind::ExtractChunk => "extract_chunk",
            JobKind::AppendBuffer => "append_buffer",
            JobKind::Seal => "seal",
            JobKind::FlushStale => "flush_stale",
            JobKind::ReembedBackfill => "reembed_backfill",
            JobKind::SealDocument => "seal_document",
        }
    }

    /// Inverse of [`Self::as_str`]; returns `Err` for unknown kinds.
    pub fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "extract_chunk" => JobKind::ExtractChunk,
            "append_buffer" => JobKind::AppendBuffer,
            "seal" => JobKind::Seal,
            "flush_stale" => JobKind::FlushStale,
            // Legacy kinds from the removed global/topic trees. They are
            // rejected on parse so a leftover queue row is recognised as
            // retired (the worker skips it, `store::purge_retired_jobs` deletes
            // it) rather than silently treated as a live kind.
            "topic_route" | "digest_daily" => {
                return Err(anyhow!(
                    "retired JobKind '{s}' (global/topic trees removed)"
                ))
            }
            "reembed_backfill" => JobKind::ReembedBackfill,
            "seal_document" => JobKind::SealDocument,
            other => return Err(anyhow!("unknown JobKind '{other}'")),
        })
    }

    /// True when handling this kind should hold a slot from the global LLM
    /// concurrency gate (see [`crate::memory::queue::gate`]).
    pub fn is_llm_bound(&self) -> bool {
        matches!(
            self,
            JobKind::ExtractChunk
                | JobKind::Seal
                | JobKind::ReembedBackfill
                | JobKind::SealDocument
        )
    }
}

/// Snake-case wire strings for the retired job kinds. Recognised so old queue
/// rows can be skipped on claim and purged, without crashing migrations.
pub const RETIRED_JOB_KINDS: [&str; 2] = ["topic_route", "digest_daily"];

/// Outcome of a successful handler run. Workers translate this into a queue
/// settlement: `Done` finalises the row, while `Defer` puts it back to `ready`
/// with `available_at_ms = until_ms` and **does not** count toward the
/// failure-attempt budget.
///
/// `Defer` exists so a handler that is transiently unable to make progress
/// (cloud rate-limited, dependency unavailable, model warming up) can re-queue
/// its job with a wake-up time without marking it failed. Handlers should still
/// surface real errors via `Err(_)` — that path runs the exponential-backoff
/// retry logic which **does** burn the failure budget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobOutcome {
    /// Handler ran to completion. Row is settled as `done`.
    Done,
    /// Handler chose not to make progress yet. Row is rescheduled to
    /// `available_at_ms = until_ms` (UTC milliseconds) with `attempts` reverted
    /// to its pre-claim value so the failure budget is not touched. `reason` is
    /// recorded in `last_error` for visibility.
    Defer {
        /// UTC-millisecond instant to reschedule the row to.
        until_ms: i64,
        /// Human-readable reason recorded in `last_error`.
        reason: String,
    },
}

/// Lifecycle states persisted on `mem_tree_jobs.status`. Workers transition
/// `ready → running → done|failed`. `Cancelled` is reserved for explicit admin
/// actions (none surfaced yet).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    /// Claimable: waiting for a worker (wire string `ready`).
    Ready,
    /// Claimed and in flight under a lease (wire string `running`).
    Running,
    /// Settled successfully (wire string `done`).
    Done,
    /// Settled as failed after exhausting retries or on an unrecoverable
    /// classification (wire string `failed`).
    Failed,
    /// Cancelled by explicit admin action (wire string `cancelled`); reserved,
    /// no producer yet.
    Cancelled,
}

impl JobStatus {
    /// Snake-case wire string written to `mem_tree_jobs.status`.
    pub fn as_str(&self) -> &'static str {
        match self {
            JobStatus::Ready => "ready",
            JobStatus::Running => "running",
            JobStatus::Done => "done",
            JobStatus::Failed => "failed",
            JobStatus::Cancelled => "cancelled",
        }
    }

    /// Inverse of [`Self::as_str`]; returns `Err` for unknown values.
    pub fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "ready" => JobStatus::Ready,
            "running" => JobStatus::Running,
            "done" => JobStatus::Done,
            "failed" => JobStatus::Failed,
            "cancelled" => JobStatus::Cancelled,
            other => return Err(anyhow!("unknown JobStatus '{other}'")),
        })
    }

    /// True for `Done`, `Failed`, `Cancelled` — i.e. no further worker
    /// transitions are expected.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            JobStatus::Done | JobStatus::Failed | JobStatus::Cancelled
        )
    }
}

/// Typed failure classification attached to a job error so the store can fail
/// fast on causes that retrying cannot fix.
///
/// Stand-in for OpenHuman's `memory_tree::health::PipelineFailure` (not part of
/// this crate's ported surface). Implements [`std::error::Error`] so handlers
/// can attach it to an `anyhow` chain; the worker downcasts it back out at
/// settle time and the store persists `code` / `class` into the typed columns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobFailure {
    /// Machine-readable cause, e.g. `"budget_exhausted"`.
    pub code: &'static str,
    /// Failure class: `"transient"` or `"unrecoverable"`.
    pub class: &'static str,
}

impl JobFailure {
    /// An unrecoverable failure: terminal on the first attempt, no retries.
    pub fn unrecoverable(code: &'static str) -> Self {
        Self {
            code,
            class: "unrecoverable",
        }
    }

    /// A transient failure: keeps the attempts-bounded retry-with-backoff path.
    pub fn transient(code: &'static str) -> Self {
        Self {
            code,
            class: "transient",
        }
    }

    /// Convenience: the canonical budget-exhausted unrecoverable failure.
    pub fn budget_exhausted() -> Self {
        Self::unrecoverable("budget_exhausted")
    }

    /// True when retrying the same input cannot succeed (fail fast).
    pub fn is_unrecoverable(&self) -> bool {
        self.class == "unrecoverable"
    }
}

impl std::fmt::Display for JobFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.code, self.class)
    }
}

impl std::error::Error for JobFailure {}

/// One row in `mem_tree_jobs`. `payload_json` is left as a raw string so
/// callers parse it lazily based on `kind`.
#[derive(Clone, Debug)]
pub struct Job {
    /// Row id (primary key in `mem_tree_jobs`).
    pub id: String,
    /// Job discriminator selecting the handler and payload shape.
    pub kind: JobKind,
    /// Raw JSON payload, parsed lazily by the handler based on `kind`.
    pub payload_json: String,
    /// Optional dedupe key backing the partial unique index that suppresses
    /// duplicate in-flight enqueues; `None` disables dedupe for this row.
    pub dedupe_key: Option<String>,
    /// Current lifecycle state.
    pub status: JobStatus,
    /// Failed attempts so far (incremented on each retryable error).
    pub attempts: u32,
    /// Attempt budget; once `attempts` reaches it the job is settled `failed`.
    pub max_attempts: u32,
    /// Earliest UTC ms at which the row may be claimed (delayed/retried work).
    pub available_at_ms: i64,
    /// Lease expiry in UTC ms while `running`; reclaimable once past.
    pub locked_until_ms: Option<i64>,
    /// Freeform last-error text for visibility; not machine-readable.
    pub last_error: Option<String>,
    /// Typed failure code (e.g. `"budget_exhausted"`) set when a job is marked
    /// `failed` with a classified reason; `None` otherwise. Distinct from the
    /// freeform `last_error` — this is the machine-readable cause.
    pub failure_reason: Option<String>,
    /// Failure class (`"transient"` | `"unrecoverable"`) paired with
    /// `failure_reason`; `None` until a classified failure is recorded.
    pub failure_class: Option<String>,
    /// Row creation time in UTC ms.
    pub created_at_ms: i64,
    /// UTC ms when the row was first claimed; `None` until it runs.
    pub started_at_ms: Option<i64>,
    /// UTC ms when the row reached a terminal status; `None` until settled.
    pub completed_at_ms: Option<i64>,
}

/// Caller-side bundle for `enqueue` — `Job` minus the persistence-only columns.
/// Keeps producers from having to mint timestamps and ids by hand.
#[derive(Clone, Debug)]
pub struct NewJob {
    /// Job discriminator selecting the handler and payload shape.
    pub kind: JobKind,
    /// Raw JSON payload, parsed lazily by the handler based on `kind`.
    pub payload_json: String,
    /// Optional dedupe key; `Some` opts the enqueue into in-flight suppression.
    pub dedupe_key: Option<String>,
    /// `None` means "available immediately." Set this for delayed jobs
    /// (retries, scheduled work).
    pub available_at_ms: Option<i64>,
    /// Attempt budget override; `None` lets the store apply its default.
    pub max_attempts: Option<u32>,
}

impl NewJob {
    /// Build a [`JobKind::ExtractChunk`] enqueue request.
    pub fn extract_chunk(p: &ExtractChunkPayload) -> Result<Self> {
        Ok(Self {
            kind: JobKind::ExtractChunk,
            payload_json: serde_json::to_string(p)?,
            dedupe_key: Some(p.dedupe_key()),
            available_at_ms: None,
            max_attempts: None,
        })
    }

    /// Build a [`JobKind::AppendBuffer`] enqueue request.
    pub fn append_buffer(p: &AppendBufferPayload) -> Result<Self> {
        Ok(Self {
            kind: JobKind::AppendBuffer,
            payload_json: serde_json::to_string(p)?,
            dedupe_key: Some(p.dedupe_key()),
            available_at_ms: None,
            max_attempts: None,
        })
    }

    /// Build a [`JobKind::Seal`] enqueue request.
    pub fn seal(p: &SealPayload) -> Result<Self> {
        Ok(Self {
            kind: JobKind::Seal,
            payload_json: serde_json::to_string(p)?,
            dedupe_key: Some(p.dedupe_key()),
            available_at_ms: None,
            max_attempts: None,
        })
    }

    /// Build a [`JobKind::FlushStale`] enqueue request scoped to a 3-hour UTC
    /// block. Callers compute `date_iso` and `hour_block` from a single
    /// `Utc::now()` reading so the dedupe key is boundary-safe.
    pub fn flush_stale(p: &FlushStalePayload, date_iso: &str, hour_block: u32) -> Result<Self> {
        Ok(Self {
            kind: JobKind::FlushStale,
            payload_json: serde_json::to_string(p)?,
            dedupe_key: Some(p.dedupe_key(date_iso, hour_block)),
            available_at_ms: None,
            max_attempts: None,
        })
    }

    /// Build a [`JobKind::ReembedBackfill`] enqueue request.
    pub fn reembed_backfill(p: &ReembedBackfillPayload) -> Result<Self> {
        Ok(Self {
            kind: JobKind::ReembedBackfill,
            payload_json: serde_json::to_string(p)?,
            dedupe_key: Some(p.dedupe_key()),
            available_at_ms: None,
            max_attempts: Some(3),
        })
    }

    /// Build a [`JobKind::SealDocument`] enqueue request.
    pub fn seal_document(p: &SealDocumentPayload) -> Result<Self> {
        Ok(Self {
            kind: JobKind::SealDocument,
            payload_json: serde_json::to_string(p)?,
            dedupe_key: Some(p.dedupe_key()),
            available_at_ms: None,
            max_attempts: None,
        })
    }
}

#[cfg(test)]
#[path = "types_tests.rs"]
mod tests;
