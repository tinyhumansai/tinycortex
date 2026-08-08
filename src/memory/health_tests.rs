//! Tests for the pipeline failure taxonomy and the embed-error classifier.

use super::*;

const ALL_CODES: [FailureCode; 11] = [
    FailureCode::BudgetExhausted,
    FailureCode::AuthMissing,
    FailureCode::AuthInvalid,
    FailureCode::EmbeddingsUnconfigured,
    FailureCode::EmbeddingDimMismatch,
    FailureCode::LocalModelUnavailable,
    FailureCode::ExtractionTimeout,
    FailureCode::SummarizerUnavailable,
    FailureCode::EmptyInputRefused,
    FailureCode::StorageUnavailable,
    FailureCode::Transient,
];

#[test]
fn every_code_has_class_and_nonempty_remediation_key() {
    for code in ALL_CODES {
        let key = code.remediation_key();
        assert!(
            !key.is_empty(),
            "{} has empty remediation key",
            code.as_str()
        );
        assert!(
            key.starts_with("memory.health.remediation."),
            "{} remediation key has unexpected prefix: {key}",
            code.as_str()
        );
        // class() must be total (no panic); Transient, ExtractionTimeout
        // and LocalModelUnavailable are retryable, everything else is
        // unrecoverable.
        let class = code.class();
        match code {
            FailureCode::Transient
            | FailureCode::ExtractionTimeout
            | FailureCode::LocalModelUnavailable => {
                assert_eq!(
                    class,
                    FailureClass::Transient,
                    "{} should be transient",
                    code.as_str()
                );
            }
            _ => {
                assert_eq!(
                    class,
                    FailureClass::Unrecoverable,
                    "{} should be unrecoverable",
                    code.as_str()
                );
            }
        }
    }
}

#[test]
fn code_str_roundtrips() {
    for code in ALL_CODES {
        assert_eq!(FailureCode::from_str(code.as_str()), Some(code));
    }
    assert_eq!(FailureCode::from_str("nonsense"), None);
}

#[test]
fn new_fills_class_and_remediation_from_code() {
    let f = PipelineFailure::new(FailureCode::BudgetExhausted);
    assert_eq!(f.code, FailureCode::BudgetExhausted);
    assert_eq!(f.class, FailureClass::Unrecoverable);
    assert_eq!(
        f.remediation_key,
        "memory.health.remediation.budget_exhausted"
    );
    assert!(f.detail.is_none());
    assert!(f.is_unrecoverable());
}

#[test]
fn with_detail_and_display() {
    let f = PipelineFailure::new(FailureCode::Transient).with_detail("HTTP 503");
    assert_eq!(f.detail.as_deref(), Some("HTTP 503"));
    assert!(!f.is_unrecoverable());
    assert_eq!(f.to_string(), "transient (transient): HTTP 503");
}

#[test]
fn pipeline_failure_serde_roundtrips() {
    let f = PipelineFailure::new(FailureCode::EmbeddingDimMismatch).with_detail("got 3072");
    let json = serde_json::to_string(&f).unwrap();
    let back: PipelineFailure = serde_json::from_str(&json).unwrap();
    assert_eq!(f, back);
    // detail omitted when None.
    let none = PipelineFailure::new(FailureCode::AuthMissing);
    assert!(!serde_json::to_string(&none).unwrap().contains("detail"));
}

#[test]
fn degraded_state_default_is_healthy() {
    let d = DegradedState::default();
    assert!(!d.is_degraded());
    let d2 = DegradedState {
        structure: true,
        ..Default::default()
    };
    assert!(d2.is_degraded());
}

#[test]
fn pipeline_failure_is_error_and_downcasts_from_anyhow() {
    let err: anyhow::Error =
        anyhow::Error::new(PipelineFailure::new(FailureCode::BudgetExhausted));
    let downcast = err.downcast_ref::<PipelineFailure>();
    assert!(downcast.is_some());
    assert!(downcast.unwrap().is_unrecoverable());
}

// ── classify_embed_error (T008) ──────────────────────────────────────

#[test]
fn classify_budget_from_body_wording() {
    // The managed Voyage route surfaces budget exhaustion in the body.
    let f = classify_embed_error_str(
        "Embedding API error (400 Bad Request): {\"error\":\"Insufficient budget\"}",
    );
    assert_eq!(f.code, FailureCode::BudgetExhausted);
    assert!(f.is_unrecoverable());
}

#[test]
fn classify_budget_from_402() {
    let f = classify_embed_error_str("Embedding API error (402 Payment Required): nope");
    assert_eq!(f.code, FailureCode::BudgetExhausted);
    assert!(f.is_unrecoverable());
}

#[test]
fn classify_429_rate_limit_as_transient() {
    let f = classify_embed_error_str("Embedding API error (429 Too Many Requests): nope");
    assert_eq!(f.code, FailureCode::Transient);
    assert!(!f.is_unrecoverable());
}

#[test]
fn classify_auth_from_401_403() {
    for status in ["401 Unauthorized", "403 Forbidden"] {
        let f = classify_embed_error_str(&format!("Embedding API error ({status}): denied"));
        assert_eq!(f.code, FailureCode::AuthInvalid, "status {status}");
        assert!(f.is_unrecoverable());
    }
}

#[test]
fn classify_dim_mismatch() {
    let f = classify_embed_error_str("cloud embedder returned 3072 dims, expected 1024");
    assert_eq!(f.code, FailureCode::EmbeddingDimMismatch);
    assert!(f.is_unrecoverable());
}

/// #13021: the provider pre-flight bail wording from both OpenAI and the
/// cloud wrapper must classify as `EmptyInputRefused` (unrecoverable) so
/// `reembed_backfill` tombstones the offending row instead of retrying
/// the same blank input forever and eventually failing the job.
#[test]
fn classify_empty_input_refusal_as_unrecoverable() {
    for msg in [
        "openai embed: refusing empty/whitespace input at index 0 of 1 (model=text-embedding-3-small)",
        "cloud embed: refusing empty/whitespace input at index 2 of 5 (model=embedding-v1)",
    ] {
        let f = classify_embed_error_str(msg);
        assert_eq!(
            f.code,
            FailureCode::EmptyInputRefused,
            "expected EmptyInputRefused for {msg:?}"
        );
        assert!(
            f.is_unrecoverable(),
            "EmptyInputRefused must be unrecoverable for {msg:?}"
        );
    }
}

/// The refusal must out-rank the dim-mismatch and budget rules even when
/// the wrapped error happens to contain those tokens — the refusal phrase
/// is the most specific signal and the only one that means "this row is
/// permanently un-embeddable", not "the provider is misbehaving".
#[test]
fn classify_empty_input_refusal_through_anyhow_context_chain() {
    let base = anyhow::anyhow!(
        "openai embed: refusing empty/whitespace input at index 0 of 1 (model=embedding-v1)"
    );
    let wrapped = base
        .context("embed summary during seal tree_id=t level=0")
        .context("reembed_backfill chunk_id=c");
    let f = classify_embed_error(&wrapped);
    assert_eq!(f.code, FailureCode::EmptyInputRefused);
    assert!(f.is_unrecoverable());
}

/// #4359: `OpenHumanCloudEmbedding::resolve_bearer` bails with "No backend
/// session for cloud embeddings ..." *before any HTTP call* when the user
/// is signed out. This must classify as `AuthMissing` (unrecoverable, "log
/// in to OpenHuman" remediation) rather than falling through to `Transient`
/// ("will retry automatically" — a loop that an auth failure can never win).
#[test]
fn classify_no_backend_session_as_auth_missing() {
    let msg = "No backend session for cloud embeddings: log in to OpenHuman, or set \
               memory.embedding_provider to \"ollama\" / \"none\" in config.toml";
    let f = classify_embed_error_str(msg);
    assert_eq!(
        f.code,
        FailureCode::AuthMissing,
        "expected AuthMissing for {msg:?}"
    );
    assert_eq!(f.class, FailureClass::Unrecoverable);
    assert_eq!(f.remediation_key, "memory.health.remediation.auth_missing");
    assert!(f.is_unrecoverable());
}

/// The match must be case-insensitive and survive `anyhow` context wrapping:
/// the bail is `.context()`-wrapped on its way up through the embed pipeline
/// (e.g. `embed_each_via_provider` adds "cloud embeddings failed"), and
/// `classify_embed_error` flattens the chain via `{err:#}`.
#[test]
fn classify_no_backend_session_through_anyhow_context_chain() {
    let base = anyhow::anyhow!(
        "No backend session for cloud embeddings: log in to OpenHuman, or set \
         memory.embedding_provider to \"ollama\" / \"none\" in config.toml"
    );
    let wrapped = base
        .context("cloud embeddings failed")
        .context("reembed_backfill chunk_id=c");
    let f = classify_embed_error(&wrapped);
    assert_eq!(f.code, FailureCode::AuthMissing);
    assert!(f.is_unrecoverable());
}

#[test]
fn classify_5xx_is_transient() {
    let f = classify_embed_error_str("Embedding API error (503 Service Unavailable): retry");
    assert_eq!(f.code, FailureCode::Transient);
    assert!(!f.is_unrecoverable());
}

#[test]
fn classify_transport_error_is_transient() {
    let f = classify_embed_error_str("error sending request for url (...): connection reset");
    assert_eq!(f.code, FailureCode::Transient);
    assert!(!f.is_unrecoverable());
}

/// #5354 — the Ollama daemon is not listening. Verbatim wording from
/// `tinyagents::harness::embeddings::ollama::OllamaEmbeddingModel::request`.
/// Note the parenthesised hint: `parse_http_status` reads the first `(`, so
/// without an explicit match this fell through to `Transient` and the panel
/// told the user to wait for a retry that can never start their daemon.
#[test]
fn classify_ollama_daemon_down_as_local_model_unavailable() {
    let f = classify_embed_error_str(
        "ollama embed request failed (is Ollama running at http://localhost:11434?): \
         error sending request for url (http://localhost:11434/api/embed)",
    );
    assert_eq!(f.code, FailureCode::LocalModelUnavailable);
    assert_eq!(
        f.remediation_key,
        "memory.health.remediation.local_model_unavailable"
    );
    // Transient so `requeue_transient_failed` resumes ingestion by itself
    // once the user starts Ollama again.
    assert!(!f.is_unrecoverable());
}

/// #5354 — the model was never pulled. `ollama_http_error` rewrites the
/// 404 into remediation prose, so the `Embedding API error (<status>)`
/// shape the status parser looks for is gone.
#[test]
fn classify_ollama_model_not_pulled_as_local_model_unavailable() {
    let f = classify_embed_error_str(
        "Ollama embedding model `bge-m3` is not installed at http://localhost:11434. \
         Run `ollama pull bge-m3` or choose an installed embedding model",
    );
    assert_eq!(f.code, FailureCode::LocalModelUnavailable);
    assert!(!f.is_unrecoverable());
}

/// The real call path wraps the provider error twice (`ProviderEmbedder`
/// adds "ollama embeddings failed", then the seal/reembed site adds its
/// own context), so the matcher must survive the flattened chain.
#[test]
fn classify_ollama_daemon_down_through_anyhow_context_chain() {
    let base = anyhow::anyhow!(
        "ollama embed request failed (is Ollama running at http://127.0.0.1:11434?): \
         tcp connect error: Connection refused (os error 61)"
    );
    let wrapped = base
        .context("ollama embeddings failed")
        .context("seal embedding failed");
    let f = classify_embed_error(&wrapped);
    assert_eq!(f.code, FailureCode::LocalModelUnavailable);
}

/// Regression guard for the matcher's blast radius: a cloud-embedder
/// transport failure carries no Ollama wording and must keep its generic
/// `Transient` code, or every network blip would start telling users to
/// install Ollama.
#[test]
fn classify_non_ollama_transport_error_stays_transient() {
    let f = classify_embed_error_str(
        "cloud embeddings failed: error sending request for url \
         (https://api.tinyhumans.ai/openai/v1/embeddings): connection reset",
    );
    assert_eq!(f.code, FailureCode::Transient);
}

#[test]
fn classify_through_anyhow_context_chain() {
    // The embed error is commonly `.context()`-wrapped on the way up;
    // the flattened `{err:#}` must still classify.
    let base = anyhow::anyhow!("Embedding API error (402 Payment Required): out of budget");
    let wrapped = base
        .context("cloud embeddings failed")
        .context("seal embed");
    let f = classify_embed_error(&wrapped);
    assert_eq!(f.code, FailureCode::BudgetExhausted);
}

#[test]
fn parse_http_status_extracts_leading_code() {
    assert_eq!(
        parse_http_status("Embedding API error (402 Payment Required): x"),
        Some(402)
    );
    assert_eq!(parse_http_status("no parens here"), None);
    assert_eq!(parse_http_status("(not a status): x"), None);
}

#[test]
fn truncate_detail_caps_length() {
    let long = "x".repeat(500);
    let out = truncate_detail(&long);
    assert!(out.chars().count() <= 201, "got {}", out.chars().count());
    assert!(out.ends_with('…'));
}
