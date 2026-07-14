use super::*;

#[test]
fn truncate_for_log_short_input_unchanged() {
    assert_eq!(truncate_for_log("hi", 10), "hi");
}

#[test]
fn truncate_for_log_long_input_appends_ellipsis() {
    let long = "x".repeat(500);
    let out = truncate_for_log(&long, 10);
    assert_eq!(out.chars().count(), 11); // 10 + "…"
    assert!(out.ends_with('…'));
}

#[tokio::test]
async fn extract_retries_on_truncated_response() {
    // First response is truncated mid-JSON (serde EOF) — a stream cutoff,
    // not a wrong-shape body. It must be treated as retryable rather than
    // silently dropped; the second (complete) response then recovers the
    // entities.
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct TruncatedThenCompleteProvider {
        calls: AtomicUsize,
    }
    #[async_trait]
    impl ChatProvider for TruncatedThenCompleteProvider {
        fn name(&self) -> &str {
            "test:truncated"
        }
        async fn chat_for_json(&self, _p: &ChatPrompt) -> anyhow::Result<String> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                // Array never closes → serde reports EOF (is_eof()).
                Ok(r#"{"entities":[{"kind":"person","text":"Alice"}"#.to_string())
            } else {
                Ok(r#"{"entities":[{"kind":"person","text":"Alice"}],"importance":0.5,"importance_reason":"r"}"#.to_string())
            }
        }
    }

    let mock = Arc::new(TruncatedThenCompleteProvider {
        calls: AtomicUsize::new(0),
    });
    let ex = LlmEntityExtractor::new(LlmExtractorConfig::default(), mock.clone());
    let out = ex.extract("Alice met Bob.").await.unwrap();
    assert_eq!(
        mock.calls.load(Ordering::SeqCst),
        2,
        "truncation should trigger a retry, not a silent drop"
    );
    assert_eq!(out.entities.len(), 1);
    assert_eq!(out.entities[0].text, "Alice");
}

#[tokio::test]
async fn extract_does_not_retry_on_wrong_shape_response() {
    // A *complete* but wrong-shape body is a serde error that is NOT EOF.
    // Unlike a mid-JSON cutoff it's deterministic and won't fix itself on
    // retry, so it must return immediately (`calls == 1`) with an empty
    // extraction.
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct WrongShapeProvider {
        calls: AtomicUsize,
    }
    #[async_trait]
    impl ChatProvider for WrongShapeProvider {
        fn name(&self) -> &str {
            "test:wrong-shape"
        }
        async fn chat_for_json(&self, _p: &ChatPrompt) -> anyhow::Result<String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            // `entities` must be an array; a scalar is a complete-input type
            // error (not a truncation), so serde reports a non-EOF error.
            Ok(r#"{"entities":123}"#.to_string())
        }
    }

    let mock = Arc::new(WrongShapeProvider {
        calls: AtomicUsize::new(0),
    });
    let ex = LlmEntityExtractor::new(LlmExtractorConfig::default(), mock.clone());
    let out = ex.extract("Alice met Bob.").await.unwrap();
    assert_eq!(
        mock.calls.load(Ordering::SeqCst),
        1,
        "a non-EOF wrong-shape response must not trigger a retry"
    );
    assert!(
        out.entities.is_empty(),
        "wrong-shape response should yield an empty extraction"
    );
}

#[test]
fn build_prompt_sets_extraction_max_tokens_cap() {
    // Extraction must cap output tokens so a credit-metered provider prices
    // the request against a realistic budget. build_prompt is the single
    // source of that cap.
    use async_trait::async_trait;
    use std::sync::Arc;

    struct NoopProvider;
    #[async_trait]
    impl ChatProvider for NoopProvider {
        fn name(&self) -> &str {
            "test:noop"
        }
        async fn chat_for_json(&self, _p: &ChatPrompt) -> anyhow::Result<String> {
            Ok("{}".into())
        }
    }

    let ex = LlmEntityExtractor::new(LlmExtractorConfig::default(), Arc::new(NoopProvider));
    let prompt = ex.build_prompt("hello");
    assert_eq!(prompt.max_tokens, Some(EXTRACTION_MAX_OUTPUT_TOKENS));
    assert_eq!(EXTRACTION_MAX_OUTPUT_TOKENS, 8192);
}

#[tokio::test]
async fn extract_does_not_retry_on_permanent_402() {
    // A 402 (the BYO provider account is out of credits) is a permanent client
    // error: retrying reproduces it. extract() must call the provider exactly
    // once and return empty.
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct InsufficientCreditsProvider {
        calls: AtomicUsize,
    }
    #[async_trait]
    impl ChatProvider for InsufficientCreditsProvider {
        fn name(&self) -> &str {
            "test:402"
        }
        async fn chat_for_json(&self, _p: &ChatPrompt) -> anyhow::Result<String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(anyhow::anyhow!(
                "myopenrouter API error (402 Payment Required): This request requires more \
                 credits, or fewer max_tokens. You requested up to 65536 tokens, but can only \
                 afford 49732."
            ))
        }
    }

    let mock = Arc::new(InsufficientCreditsProvider {
        calls: AtomicUsize::new(0),
    });
    let ex = LlmEntityExtractor::new(LlmExtractorConfig::default(), mock.clone());
    let out = ex.extract("some text").await.unwrap();

    assert_eq!(
        mock.calls.load(Ordering::SeqCst),
        1,
        "a permanent 402 must not be retried"
    );
    assert!(out.entities.is_empty());
}

#[tokio::test]
async fn extract_does_not_retry_on_500_wrapped_monthly_quota() {
    // A 500-envelope wrapping an inner 402 monthly-quota refusal is still a
    // permanent error (MONTHLY_REQUEST_COUNT) — it must call the provider
    // exactly once.
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct MonthlyQuotaProvider {
        calls: AtomicUsize,
    }
    #[async_trait]
    impl ChatProvider for MonthlyQuotaProvider {
        fn name(&self) -> &str {
            "test:kiro"
        }
        async fn chat_for_json(&self, _p: &ChatPrompt) -> anyhow::Result<String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(anyhow::anyhow!(
                "kiro API error (500 Internal Server Error): HTTP 402 from Kiro IDE: \
                 You have reached the limit. reason: MONTHLY_REQUEST_COUNT"
            ))
        }
    }

    let mock = Arc::new(MonthlyQuotaProvider {
        calls: AtomicUsize::new(0),
    });
    let ex = LlmEntityExtractor::new(LlmExtractorConfig::default(), mock.clone());
    let out = ex.extract("some text").await.unwrap();

    assert_eq!(
        mock.calls.load(Ordering::SeqCst),
        1,
        "a 500-wrapped monthly-quota refusal must not be retried"
    );
    assert!(out.entities.is_empty());
}

#[tokio::test]
async fn extract_retries_transient_provider_error() {
    // A transport/transient failure (no 4xx, no auth marker) must still exhaust
    // the retry budget before falling back.
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct TransientProvider {
        calls: AtomicUsize,
    }
    #[async_trait]
    impl ChatProvider for TransientProvider {
        fn name(&self) -> &str {
            "test:transient"
        }
        async fn chat_for_json(&self, _p: &ChatPrompt) -> anyhow::Result<String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(anyhow::anyhow!(
                "error sending request for url (https://api): connection refused"
            ))
        }
    }

    let mock = Arc::new(TransientProvider {
        calls: AtomicUsize::new(0),
    });
    let ex = LlmEntityExtractor::new(LlmExtractorConfig::default(), mock.clone());
    let out = ex.extract("some text").await.unwrap();

    assert_eq!(
        mock.calls.load(Ordering::SeqCst),
        3,
        "a transient error must still exhaust the retry budget"
    );
    assert!(out.entities.is_empty());
}
