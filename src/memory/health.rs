//! Typed failure + degradation model for the memory pipeline.
//!
//! The chunk→wiki pipeline and the time-tree summarizer fail in several
//! distinct ways (budget exhausted, missing/invalid key, missing local
//! model, dimension mismatch, extraction timeout, transient network).
//! Historically these all collapsed into an opaque error string and were
//! retried identically — so a hard "Insufficient budget" 4xx burned the
//! retry budget and the user saw a generic `error: N failed jobs`.
//!
//! This module is the single source of truth that fixes that:
//!
//! - [`FailureCode`] enumerates every distinguishable cause.
//! - Each code maps to a [`FailureClass`] (`Transient` ⇒ retry with
//!   backoff, `Unrecoverable` ⇒ fail fast) and a stable i18n
//!   `remediation_key` so the status surface / doctor / job row all show
//!   consistent, actionable text. Embeddings remediation leads with the
//!   local-Ollama path (the steered primary fix), with BYO key secondary.
//! - [`PipelineFailure`] is a `std::error::Error`, so it can be wrapped in
//!   `anyhow` and propagated up through the job processor, then downcast in
//!   the queue worker to decide retry-vs-fail.
//! - [`DegradedState`] captures "the pipeline ran but recall/structure is
//!   reduced" — surfaced so degraded output is never presented as success.
//!
//! ## Scope: taxonomy only
//!
//! This is the engine's own failure vocabulary and nothing else. The
//! *process-global degradation flags* that the embed/extract stages set and the
//! `pipeline_status` RPC reads stay in the embedding host, because they are
//! coupled to a host socket broadcast and to host plumbing. So does the
//! `doctor` report (it reads the host's scheduler-gate config) and the
//! user-error publisher (its `kind` string is a pinned frontend contract).
//!
//! Note the name: `tinycortex_api::health` is a *different* thing — driver
//! liveness (`MemoryHealth`). This module is pipeline failure classification.
//!
//! The `remediation_key` values are i18n keys resolved by the host's frontend.
//! They travel with [`FailureCode`] because `PipelineFailure::remediation_key`
//! is a serialized wire field populated by `PipelineFailure::new`; splitting the
//! table out would change the type's shape. The emitted strings are unchanged.

use serde::{Deserialize, Serialize};
use std::fmt;


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

impl FailureClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Transient => "transient",
            Self::Unrecoverable => "unrecoverable",
        }
    }
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

impl FailureCode {
    /// Stable wire string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BudgetExhausted => "budget_exhausted",
            Self::AuthMissing => "auth_missing",
            Self::AuthInvalid => "auth_invalid",
            Self::EmbeddingsUnconfigured => "embeddings_unconfigured",
            Self::EmbeddingDimMismatch => "embedding_dim_mismatch",
            Self::LocalModelUnavailable => "local_model_unavailable",
            Self::ExtractionTimeout => "extraction_timeout",
            Self::SummarizerUnavailable => "summarizer_unavailable",
            Self::EmptyInputRefused => "empty_input_refused",
            Self::StorageUnavailable => "storage_unavailable",
            Self::Transient => "transient",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "budget_exhausted" => Self::BudgetExhausted,
            "auth_missing" => Self::AuthMissing,
            "auth_invalid" => Self::AuthInvalid,
            "embeddings_unconfigured" => Self::EmbeddingsUnconfigured,
            "embedding_dim_mismatch" => Self::EmbeddingDimMismatch,
            "local_model_unavailable" => Self::LocalModelUnavailable,
            "extraction_timeout" => Self::ExtractionTimeout,
            "summarizer_unavailable" => Self::SummarizerUnavailable,
            "empty_input_refused" => Self::EmptyInputRefused,
            "storage_unavailable" => Self::StorageUnavailable,
            "transient" => Self::Transient,
            _ => return None,
        })
    }

    /// Retry policy for this cause.
    ///
    /// [`LocalModelUnavailable`](Self::LocalModelUnavailable) is deliberately
    /// **transient** even though the user has to act: the condition (Ollama
    /// daemon stopped, model not pulled) clears from outside the app, and only
    /// transient rows are picked up by `requeue_transient_failed` — the
    /// automatic self-healing requeue. Classifying it unrecoverable would park
    /// every affected job until someone clicks "Retry failed" by hand, so a
    /// user who simply restarts Ollama would never see ingestion resume.
    pub fn class(self) -> FailureClass {
        match self {
            Self::Transient | Self::ExtractionTimeout | Self::LocalModelUnavailable => {
                FailureClass::Transient
            }
            _ => FailureClass::Unrecoverable,
        }
    }

    /// i18n key for the user-facing remediation. Embeddings causes lead
    /// with the local-Ollama path (the steered primary fix per spec FR-015).
    pub fn remediation_key(self) -> &'static str {
        match self {
            Self::BudgetExhausted => "memory.health.remediation.budget_exhausted",
            Self::AuthMissing => "memory.health.remediation.auth_missing",
            Self::AuthInvalid => "memory.health.remediation.auth_invalid",
            Self::EmbeddingsUnconfigured => "memory.health.remediation.embeddings_unconfigured",
            Self::EmbeddingDimMismatch => "memory.health.remediation.embedding_dim_mismatch",
            Self::LocalModelUnavailable => "memory.health.remediation.local_model_unavailable",
            Self::ExtractionTimeout => "memory.health.remediation.extraction_timeout",
            Self::SummarizerUnavailable => "memory.health.remediation.summarizer_unavailable",
            Self::EmptyInputRefused => "memory.health.remediation.empty_input_refused",
            Self::StorageUnavailable => "memory.health.remediation.storage_unavailable",
            Self::Transient => "memory.health.remediation.transient",
        }
    }
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

impl PipelineFailure {
    /// Build a failure from a code, deriving class + remediation key.
    pub fn new(code: FailureCode) -> Self {
        Self {
            code,
            class: code.class(),
            remediation_key: code.remediation_key().to_string(),
            detail: None,
        }
    }

    /// Attach a non-localized detail string (truncated by callers; never
    /// log secrets).
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// True when this failure should fail fast (no retry budget).
    pub fn is_unrecoverable(&self) -> bool {
        self.class == FailureClass::Unrecoverable
    }
}

/// Classify an embedding-stage error into a typed [`PipelineFailure`].
///
/// The embed path bottoms out in `embeddings::openai::OpenAiEmbedding::embed`,
/// which on a non-2xx response bails with the message
/// `"Embedding API error (<status>): <body>"` (status is reqwest's
/// `StatusCode` Display, e.g. `402 Payment Required`). Dimension mismatches
/// surface from the memory-tree `CloudEmbedder`/trait validator as
/// `"... returned N dims, expected M"` or `"... dims, expected ..."`. We
/// parse those shapes to decide retry-vs-fail:
///
/// - `401` / `403` → `auth_invalid` (a bearer was sent but rejected).
/// - `402` / `429` / a body mentioning budget/quota/insufficient →
///   `budget_exhausted` (the managed Voyage route is out of budget; the
///   user must bring their own key or top up — retrying won't help).
/// - dimension-mismatch text → `embedding_dim_mismatch`.
/// - Ollama daemon-unreachable / model-not-pulled text →
///   `local_model_unavailable`, so the panel names the local-runtime fix.
/// - everything else (5xx, timeouts, transport, unparseable) → `transient`,
///   so the worker's existing retry-with-backoff still applies.
///
/// Operates on the flattened `anyhow` chain (`{err:#}`) so it still matches
/// when the embed error has been `.context()`-wrapped on the way up.
pub fn classify_embed_error(err: &anyhow::Error) -> PipelineFailure {
    let msg = format!("{err:#}");
    classify_embed_error_str(&msg)
}

/// String-level core of [`classify_embed_error`], split out so unit tests can
/// exercise the mapping without constructing reqwest errors.
pub fn classify_embed_error_str(msg: &str) -> PipelineFailure {
    let lower = msg.to_ascii_lowercase();

    // #13021: client-side refusal from the provider pre-flight guard fires
    // *before* any HTTP round-trip, so it carries no `Embedding API error
    // (<status>)` shape. Without an explicit match it would fall through to
    // `Transient` and the `reembed_backfill` worker would retry the same
    // un-embeddable row forever (and eventually fail the whole job).
    // Classify as unrecoverable per-row so the worker tombstones the chunk /
    // summary instead. Both `OpenAiEmbedding::embed` and
    // `OpenHumanCloudEmbedding::embed` use the literal phrase
    // "refusing empty/whitespace".
    if lower.contains("refusing empty/whitespace") {
        return PipelineFailure::new(FailureCode::EmptyInputRefused)
            .with_detail(truncate_detail(msg));
    }

    // Sibling of the #13021 case above: `OpenHumanCloudEmbedding::resolve_bearer`
    // bails *before any HTTP round-trip* when the desktop/backend session
    // bearer is absent (user signed out), with the literal phrase
    // "No backend session for cloud embeddings ..." (see
    // `src/openhuman/inference/embeddings/cloud.rs`). Being a client-side bail it carries
    // no `Embedding API error (<status>)` shape, so without this match it falls
    // through to `Transient` — the Memory Tree then shows "temporary error…
    // will retry automatically" and the worker retries an auth failure that a
    // retry can never fix. Classify as `AuthMissing` so the health banner
    // surfaces the "log in to OpenHuman" remediation and the job fails fast.
    if lower.contains("no backend session") {
        return PipelineFailure::new(FailureCode::AuthMissing).with_detail(truncate_detail(msg));
    }

    // #5354 — the local Ollama runtime is not usable: the daemon is not
    // listening, or the configured embedding model was never pulled. Both are
    // emitted by `tinyagents::harness::embeddings::ollama` with the fix already
    // in the text:
    //
    //   "ollama embed request failed (is Ollama running at <base>?): …"
    //   "Ollama embedding model `<id>` is not installed at <base>. Run `ollama pull <id>` …"
    //
    // Neither carries an `Embedding API error (<status>)` shape — the first is a
    // transport bail, the second a rewritten 404 — so both used to fall through
    // to `Transient` and surface as "a temporary error … will retry
    // automatically". That is the wrong remediation: retrying cannot start a
    // daemon or pull a model, and the user was never told what to do. Match the
    // two shapes explicitly so the status panel renders the
    // `local_model_unavailable` remediation instead. The class stays transient
    // (see `FailureCode::class`) so jobs auto-resume once Ollama is back.
    //
    // Anchored on Ollama-specific wording so a generic cloud-embedder transport
    // failure ("error sending request for url …") keeps its `Transient` code.
    if lower.contains("is ollama running at")
        || (lower.contains("ollama embedding model") && lower.contains("is not installed at"))
    {
        return PipelineFailure::new(FailureCode::LocalModelUnavailable)
            .with_detail(truncate_detail(msg));
    }

    // Dimension mismatch — the trait validator / CloudEmbedder rejects a
    // vector whose length isn't EMBEDDING_DIM. Check before status parsing:
    // it's a 2xx-but-wrong-shape case with no HTTP status to match.
    if lower.contains("dims, expected") || lower.contains("dimensions, expected") {
        return PipelineFailure::new(FailureCode::EmbeddingDimMismatch)
            .with_detail(truncate_detail(msg));
    }

    // Budget/quota wording wins regardless of the numeric status — the
    // managed backend may surface budget exhaustion as 4xx with an explicit
    // body, and we always want the BYO-key remediation here.
    if lower.contains("insufficient budget")
        || lower.contains("budget")
        || lower.contains("quota")
        || lower.contains("payment required")
    {
        return PipelineFailure::new(FailureCode::BudgetExhausted)
            .with_detail(truncate_detail(msg));
    }

    // Parse the HTTP status out of the `Embedding API error (<status>): ...`
    // shape. reqwest renders e.g. `402 Payment Required`, so the first
    // 3-digit run after the opening paren is the code.
    if let Some(code) = parse_http_status(msg) {
        return match code {
            401 | 403 => {
                PipelineFailure::new(FailureCode::AuthInvalid).with_detail(truncate_detail(msg))
            }
            402 => {
                PipelineFailure::new(FailureCode::BudgetExhausted).with_detail(truncate_detail(msg))
            }
            429 => PipelineFailure::new(FailureCode::Transient).with_detail(truncate_detail(msg)),
            // 4xx other than the above is a hard client error retrying won't
            // fix (malformed request, model not found); fail fast but tag it
            // generically as auth_invalid's sibling — use Transient only for
            // 5xx/unknown. We treat unknown 4xx as unrecoverable via
            // budget? No — be conservative: only the known codes above are
            // unrecoverable; other 4xx fall through to transient so we don't
            // wedge on a transient 408/425.
            500..=599 => {
                PipelineFailure::new(FailureCode::Transient).with_detail(truncate_detail(msg))
            }
            _ => PipelineFailure::new(FailureCode::Transient).with_detail(truncate_detail(msg)),
        };
    }

    // No recognizable status — transport error, timeout, connection reset,
    // or an unparseable message. Treat as transient so retry/backoff applies.
    PipelineFailure::new(FailureCode::Transient).with_detail(truncate_detail(msg))
}

/// Extract the first HTTP status code from an `Embedding API error (<status>)`
/// message. Returns the leading 3-digit number inside the first parenthesised
/// group, if present.
fn parse_http_status(msg: &str) -> Option<u16> {
    let open = msg.find('(')?;
    let rest = &msg[open + 1..];
    let digits: String = rest
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if digits.len() == 3 {
        digits.parse().ok()
    } else {
        None
    }
}

/// Cap a detail string so we never balloon logs / wire payloads with a full
/// provider response body. Never contains a secret (it's an error body), but
/// keep it short anyway.
fn truncate_detail(s: &str) -> String {
    const MAX: usize = 200;
    if s.chars().count() <= MAX {
        return s.to_string();
    }
    let truncated: String = s.chars().take(MAX).collect();
    format!("{truncated}…")
}

impl fmt::Display for PipelineFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.code.as_str(), self.class.as_str())?;
        if let Some(detail) = &self.detail {
            write!(f, ": {detail}")?;
        }
        Ok(())
    }
}

impl std::error::Error for PipelineFailure {}

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

impl DegradedState {
    /// True when any degradation is present.
    pub fn is_degraded(&self) -> bool {
        self.semantic_recall || self.structure || self.storage
    }
}

#[cfg(test)]
#[path = "health_tests.rs"]
mod tests;
