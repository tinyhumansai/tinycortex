//! Wiremock suite for the OpenRouter reference provider (§6.6): happy path,
//! JSON-mode digest, 429 retry, 402 fail-fast, budget cutoff, embeddings.

use super::*;
use std::time::Duration;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::memory::score::extract::ChatPrompt;

fn cfg(base_url: String) -> OpenRouterConfig {
    OpenRouterConfig {
        base_url,
        api_key: SecretString::new("sk-test"),
        request_timeout: Duration::from_secs(5),
        max_attempts: 4,
        ..Default::default()
    }
}

fn chat_completion_body(content: &str) -> serde_json::Value {
    json!({
        "id": "gen-1",
        "choices": [{"index": 0, "message": {"role": "assistant", "content": content}}],
        "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15, "cost": 0.0001}
    })
}

fn prompt(user: &str) -> ChatPrompt {
    ChatPrompt {
        system: "Return JSON only.".into(),
        user: user.into(),
        temperature: 0.0,
        kind: "persona::digest",
        max_tokens: Some(256),
    }
}

#[tokio::test]
async fn chat_for_json_happy_path_returns_content_and_records_usage() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(chat_completion_body("{\"ok\":true}")))
        .mount(&server)
        .await;

    let provider = OpenRouterProvider::new(cfg(format!("{}/", server.uri()))).unwrap();
    let out = provider.chat_for_json(&prompt("hi")).await.unwrap();
    assert_eq!(out, "{\"ok\":true}");

    let usage = provider.usage();
    assert_eq!(usage.requests, 1);
    assert_eq!(usage.prompt_tokens, 10);
    assert!((usage.cost_usd - 0.0001).abs() < 1e-9);
}

#[tokio::test]
async fn retries_429_then_succeeds() {
    let server = MockServer::start().await;
    // First response 429 (higher priority, single use), then the 200 fallback.
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(429).set_body_string("rate limited"))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(chat_completion_body("{\"ok\":1}")))
        .with_priority(2)
        .mount(&server)
        .await;

    let provider = OpenRouterProvider::new(cfg(server.uri())).unwrap();
    let out = provider.chat_for_json(&prompt("hi")).await.unwrap();
    assert_eq!(out, "{\"ok\":1}");
    assert_eq!(provider.usage().requests, 1, "only the successful call counts");
}

#[tokio::test]
async fn fails_fast_on_402_without_retry() {
    let server = MockServer::start().await;
    let mock = Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(402).set_body_string("requires more credits"))
        .expect(1) // must NOT retry a 4xx
        .mount_as_scoped(&server)
        .await;

    let provider = OpenRouterProvider::new(cfg(server.uri())).unwrap();
    let err = provider.chat_for_json(&prompt("hi")).await.unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("402"), "error should carry the status: {msg}");
    drop(mock); // verifies expect(1)
}

#[tokio::test]
async fn run_cost_budget_cuts_off_further_calls() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(chat_completion_body("{\"a\":1}")))
        .mount(&server)
        .await;

    let mut c = cfg(server.uri());
    c.run_cost_limit_usd = Some(0.00005); // one call (0.0001) exceeds this
    let provider = OpenRouterProvider::new(c).unwrap();

    // First call succeeds and pushes cumulative cost over the ceiling.
    provider.chat_for_json(&prompt("one")).await.unwrap();
    // Second call is refused before it hits the network.
    let err = provider.chat_for_json(&prompt("two")).await.unwrap_err();
    assert!(format!("{err:#}").contains("budget exhausted"));
    assert_eq!(provider.usage().requests, 1);
}

#[tokio::test]
async fn embeddings_return_vectors_in_order() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [
                {"index": 1, "embedding": [0.1, 0.2, 0.3]},
                {"index": 0, "embedding": [0.4, 0.5, 0.6]}
            ],
            "usage": {"prompt_tokens": 4, "total_tokens": 4, "cost": 0.00002}
        })))
        .mount(&server)
        .await;

    let mut c = cfg(server.uri());
    c.embed_dims = 3;
    let provider = OpenRouterProvider::new(c).unwrap();
    let vecs = EmbeddingBackend::embed(&provider, &["a", "b"]).await.unwrap();
    assert_eq!(vecs.len(), 2);
    // index-0 vector must land first even though it arrived second.
    assert_eq!(vecs[0], vec![0.4, 0.5, 0.6]);
    assert_eq!(vecs[1], vec![0.1, 0.2, 0.3]);
    assert!(EmbeddingBackend::signature(&provider).contains("provider=openrouter"));
}

#[tokio::test]
async fn summariser_folds_via_chat_and_reports_usage() {
    use crate::memory::tree::store::TreeKind;
    use chrono::Utc;

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(chat_completion_body("Prefers small commits.")),
        )
        .mount(&server)
        .await;

    let provider = OpenRouterProvider::new(cfg(server.uri())).unwrap();
    let now = Utc::now();
    let inputs = vec![SummaryInput {
        id: "e1".into(),
        content: "user: commit small and often".into(),
        token_count: 8,
        entities: vec![],
        topics: vec![],
        time_range_start: now,
        time_range_end: now,
        score: 0.9,
    }];
    let ctx = SummaryContext {
        tree_id: "t1",
        tree_kind: TreeKind::Flavoured,
        target_level: 1,
        token_budget: 200,
        ask: Some("Distill workflow habits."),
    };
    let call = Summariser::summarise_with_usage(&provider, &inputs, &ctx)
        .await
        .unwrap();
    assert!(call.output.content.contains("small commits"));
    assert_eq!(call.input_tokens, 10);
    assert_eq!(call.output_tokens, 5);
}
