//! Type definitions for the pipeline failure + degradation taxonomy.
//!
//! The taxonomy itself — `FailureClass`, `FailureCode`, `PipelineFailure`,
//! `DegradedState` — is data: serde types carried on the wire and embedded in
//! `PipelineFailure`. The classification logic (`classify_embed_error`), the
//! `std::error::Error` plumbing, and the remediation-key/class derivation live
//! in the parent `health` module, so this file stays pure type definitions.

use serde::{Deserialize, Serialize};

/// Whether a failure should be retried (`Transient`) or fail fast
/// (`Unrecoverable`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureClass {
    /// Retry with backoff up to `max_attempts` (network 5xx, timeouts,
    /// truncated streams).
    Transient,
    /// Stop immediately — retrying the same input cannot succeed (budget
    /// exhausted, bad/missing key, missing local model, dim mismatch).
    Unrecoverable,
}

/// A distinguishable pipeline failure cause. Each variant carries a fixed
/// [`FailureClass`] and i18n remediation key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureCode {
    /// Managed embeddings route returned an out-of-budget error (4xx).
    BudgetExhausted,
    /// No auth/session available for the embeddings provider.
    AuthMissing,
    /// Auth present but rejected (expired/invalid key or JWT).
    AuthInvalid,
    /// No embeddings provider is configured at all.
    EmbeddingsUnconfigured,
    /// Provider returned vectors of an unexpected dimensionality.
    EmbeddingDimMismatch,
    /// A required local model (Ollama) is not available.
    LocalModelUnavailable,
    /// The extraction model timed out / exhausted retries.
    ExtractionTimeout,
    /// No summarization provider could be resolved for "Build Summary Trees"
    /// — neither local AI nor a configured cloud chat provider. Distinct from
    /// [`LocalModelUnavailable`](Self::LocalModelUnavailable), which implies the
    /// local path was selected; this covers the cloud-only setup whose provider
    /// failed to resolve, so the remediation names both paths.
    SummarizerUnavailable,
    /// The embedding provider refused an empty/whitespace input at the
    /// pre-flight guard (#13021). Unrecoverable per-row: the offending row
    /// will never become embeddable, so the worker must tombstone it instead
    /// of retrying. Bail wording for both `OpenAiEmbedding::embed` and
    /// `OpenHumanCloudEmbedding::embed` starts with
    /// `"<name> embed: refusing empty/whitespace input ..."`.
    EmptyInputRefused,
    /// The host filesystem cannot service the memory_tree path — `create_dir`
    /// / DB open returned a persistent OS-level I/O error (EIO `5`, ENOSPC
    /// `28`, EROFS `30`), e.g. a failing/disconnected SD card or a volume the
    /// kernel remounted read-only. Unrecoverable from inside the app: only the
    /// user can reseat/replace/free the storage. Distinct from the embeddings
    /// provider faults above and from the SQLite-level `SQLITE_FULL` /
    /// `SQLITE_CORRUPT` handled in the queue worker — this is the
    /// directory/DB-init layer below them.
    StorageUnavailable,
    /// Catch-all transient failure (network 5xx, timeout, truncated JSON).
    Transient,
}

/// A typed pipeline failure: a [`FailureCode`] plus the derived class +
/// remediation key (carried on the wire so the frontend stays
/// presentational) and an optional human-readable detail for logs/diagnosis.
///
/// Implements [`std::error::Error`] so it can be `anyhow`-wrapped at the
/// embed/extract/summarize boundary, propagated through the job processor,
/// and downcast in the queue worker to drive retry-vs-fail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineFailure {
    pub code: FailureCode,
    pub class: FailureClass,
    /// i18n key — the frontend resolves this to localized remediation text.
    pub remediation_key: String,
    /// Optional non-localized detail for logs/diagnosis (never a secret).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// "The pipeline ran, but output quality is reduced." Surfaced so degraded
/// results are never presented as success.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DegradedState {
    /// True when embeddings were skipped (no usable provider) so semantic
    /// recall falls back to recency-only.
    pub semantic_recall: bool,
    /// True when extraction yielded empty across the board so the wiki has
    /// no entity/topic structure.
    pub structure: bool,
    /// True when the memory_tree's own storage path is unusable — the host
    /// filesystem returned a persistent I/O error on dir-create / DB open
    /// (EIO/ENOSPC/EROFS). This is the most severe degradation: the pipeline
    /// can't even open its DB, so nothing else runs. `#[serde(default)]` keeps
    /// the wire format backward-compatible (older clients omit it → `false`).
    #[serde(default)]
    pub storage: bool,
    /// The cause of the most significant degradation, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause: Option<PipelineFailure>,
}
