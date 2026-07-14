//! OpenRouter reference provider (doc 06 §6.6, closes the C3/M3 seam).
//!
//! The crate's **first** concrete LLM provider — a *reference implementation*,
//! not a hard dependency. [`OpenRouterProvider`] implements all three provider
//! seams against OpenRouter's OpenAI-compatible endpoints:
//!
//! - [`ChatProvider`] — `POST /chat/completions` in JSON mode (persona digests).
//! - [`Summariser`] — the same endpoint in plain-text mode (flavoured-tree folds).
//! - [`EmbeddingBackend`] — `POST /embeddings` (vector retrieval).
//!
//! Nothing under `memory::persona` names this type: the pipeline depends only on
//! the traits, and OpenHuman injects its own routes. Secrets are held as
//! [`SecretString`]; token usage is accumulated per-run and the provider aborts
//! cleanly (returns a non-retryable error) once a configured cost/call budget is
//! hit, mirroring the [`DailyBudget`](crate::memory::sync::state::DailyBudget)
//! pattern. Transport and `429`/`5xx` failures retry with backoff; `4xx` client
//! errors (`401`/`402`/`403`) fail fast so the pipeline's
//! `is_non_retryable` classifier short-circuits them.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::json;

use crate::memory::config::SecretString;
use crate::memory::score::extract::{ChatPrompt, ChatProvider};
use crate::memory::store::vectors::{format_embedding_signature, EmbeddingBackend};
use crate::memory::tree::{
    finish_provider_summary, prepare_summary_prompt, SummaryCall, SummaryContext, SummaryInput,
    SummaryOutput, Summariser,
};

/// Default OpenRouter API base.
pub const DEFAULT_BASE_URL: &str = "https://openrouter.ai/api/v1";
/// Default chat/digest model (small, fast, cheap).
pub const DEFAULT_CHAT_MODEL: &str = "deepseek/deepseek-v4-flash";
/// Default embedding model (OpenAI-compatible, 1536 dims).
pub const DEFAULT_EMBED_MODEL: &str = "openai/text-embedding-3-small";
/// Default embedding dimensionality for [`DEFAULT_EMBED_MODEL`].
pub const DEFAULT_EMBED_DIMS: usize = 1536;

/// Declarative configuration for [`OpenRouterProvider`].
#[derive(Clone, Debug)]
pub struct OpenRouterConfig {
    /// API base, e.g. `https://openrouter.ai/api/v1`.
    pub base_url: String,
    /// Bearer key. Never serialized or printed.
    pub api_key: SecretString,
    /// Chat/digest model id.
    pub chat_model: String,
    /// Embedding model id.
    pub embed_model: String,
    /// Embedding dimensionality (declared, must match the model).
    pub embed_dims: usize,
    /// Per-request timeout.
    pub request_timeout: Duration,
    /// Max attempts per request (1 = no retry).
    pub max_attempts: u32,
    /// Optional hard per-run USD ceiling; the provider aborts once exceeded.
    pub run_cost_limit_usd: Option<f64>,
    /// Optional hard per-run request-count ceiling.
    pub run_call_limit: Option<u32>,
}

impl Default for OpenRouterConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.to_string(),
            api_key: SecretString::default(),
            chat_model: DEFAULT_CHAT_MODEL.to_string(),
            embed_model: DEFAULT_EMBED_MODEL.to_string(),
            embed_dims: DEFAULT_EMBED_DIMS,
            request_timeout: Duration::from_secs(120),
            max_attempts: 4,
            run_cost_limit_usd: None,
            run_call_limit: None,
        }
    }
}

/// Per-run usage accumulator (chat + embeddings), mirroring the `DailyBudget`
/// pattern but denominated in USD and request count.
#[derive(Clone, Debug, Default)]
pub struct RunUsage {
    /// Total requests that reached the provider (each retry counts once).
    pub requests: u32,
    /// Total prompt tokens reported by `usage`.
    pub prompt_tokens: u64,
    /// Total completion tokens reported by `usage`.
    pub completion_tokens: u64,
    /// Total USD cost reported by `usage.cost`.
    pub cost_usd: f64,
}

/// OpenRouter-backed provider implementing all three provider seams.
pub struct OpenRouterProvider {
    http: reqwest::Client,
    cfg: OpenRouterConfig,
    usage: Arc<Mutex<RunUsage>>,
}

impl OpenRouterProvider {
    /// Build a provider from config. Fails only if the underlying HTTP client
    /// cannot be constructed.
    pub fn new(cfg: OpenRouterConfig) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(cfg.request_timeout)
            .build()
            .context("build OpenRouter HTTP client")?;
        Ok(Self {
            http,
            cfg,
            usage: Arc::new(Mutex::new(RunUsage::default())),
        })
    }

    /// Snapshot of accumulated per-run usage.
    pub fn usage(&self) -> RunUsage {
        self.usage.lock().clone()
    }

    /// The configured chat model id.
    pub fn chat_model(&self) -> &str {
        &self.cfg.chat_model
    }

    /// Fail-closed budget check run *before* every request. Returns a
    /// non-retryable error once a configured ceiling is hit so the pipeline
    /// checkpoints and stops cleanly.
    fn check_budget(&self) -> Result<()> {
        let u = self.usage.lock();
        if let Some(limit) = self.cfg.run_call_limit {
            if u.requests >= limit {
                return Err(anyhow!(
                    "run budget exhausted: request limit {limit} reached (non-retryable)"
                ));
            }
        }
        if let Some(limit) = self.cfg.run_cost_limit_usd {
            if u.cost_usd >= limit {
                return Err(anyhow!(
                    "run budget exhausted: cost limit ${limit:.4} reached (non-retryable)"
                ));
            }
        }
        Ok(())
    }

    /// Record the `usage` block of a successful response.
    fn record_usage(&self, usage: &serde_json::Value) {
        let mut u = self.usage.lock();
        u.requests += 1;
        u.prompt_tokens += usage.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
        u.completion_tokens += usage
            .get("completion_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        if let Some(cost) = usage.get("cost").and_then(|v| v.as_f64()) {
            if cost.is_finite() && cost > 0.0 {
                u.cost_usd += cost;
            }
        }
    }

    /// POST `body` to `{base}/{path}` with retry/backoff and return the parsed
    /// JSON response. Retries transport failures and `429`/`5xx`; fails fast on
    /// `4xx` client errors with the status embedded in the error string.
    async fn post_json(&self, path: &str, body: serde_json::Value) -> Result<serde_json::Value> {
        self.check_budget()?;
        let url = format!("{}/{path}", self.cfg.base_url.trim_end_matches('/'));
        let mut last_err: Option<anyhow::Error> = None;

        for attempt in 0..self.cfg.max_attempts.max(1) {
            if attempt > 0 {
                // Deterministic exponential backoff (no RNG): 400ms, 800ms, …
                let backoff = 400u64.saturating_mul(1 << (attempt.min(5) - 1));
                tokio::time::sleep(Duration::from_millis(backoff)).await;
            }

            let resp = self
                .http
                .post(&url)
                .bearer_auth(self.cfg.api_key.expose())
                .header("HTTP-Referer", "https://github.com/tinyhumansai/tinycortex")
                .header("X-Title", "TinyCortex Persona")
                .json(&body)
                .send()
                .await;

            match resp {
                Ok(r) => {
                    let status = r.status();
                    let text = r.text().await.unwrap_or_default();
                    if status.is_success() {
                        return serde_json::from_str(&text)
                            .with_context(|| format!("decode OpenRouter {path} response"));
                    }
                    let code = status.as_u16();
                    let snippet: String = text.chars().take(300).collect();
                    // 4xx (except 408/429) are permanent client errors — fail fast.
                    let retryable = code == 408 || code == 429 || (500..600).contains(&code);
                    let err = anyhow!("OpenRouter {path} HTTP {code}: {snippet}");
                    if !retryable {
                        return Err(err);
                    }
                    last_err = Some(err);
                }
                Err(e) => {
                    // Transport failure — always retryable.
                    last_err = Some(anyhow!("OpenRouter {path} transport error: {e}"));
                }
            }
        }

        Err(last_err.unwrap_or_else(|| anyhow!("OpenRouter {path} failed with no error")))
    }

    /// Run one chat completion. When `json_mode` is set the request asks for a
    /// JSON object response. Returns the assistant message content.
    async fn chat(
        &self,
        system: &str,
        user: &str,
        temperature: f32,
        max_tokens: Option<u32>,
        json_mode: bool,
    ) -> Result<String> {
        let mut body = json!({
            "model": self.cfg.chat_model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user},
            ],
            "temperature": temperature,
        });
        if let Some(mt) = max_tokens {
            body["max_tokens"] = json!(mt);
        }
        if json_mode {
            body["response_format"] = json!({"type": "json_object"});
        }

        let resp = self.post_json("chat/completions", body).await?;
        if let Some(usage) = resp.get("usage") {
            self.record_usage(usage);
        }
        let content = resp
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .ok_or_else(|| anyhow!("OpenRouter chat response missing choices[0].message.content"))?;
        Ok(content.to_string())
    }
}

#[async_trait]
impl ChatProvider for OpenRouterProvider {
    fn name(&self) -> &str {
        "openrouter"
    }

    async fn chat_for_json(&self, prompt: &ChatPrompt) -> Result<String> {
        self.chat(
            &prompt.system,
            &prompt.user,
            prompt.temperature,
            prompt.max_tokens,
            true,
        )
        .await
    }
}

#[async_trait]
impl Summariser for OpenRouterProvider {
    fn name(&self) -> &str {
        "openrouter"
    }

    async fn summarise(
        &self,
        inputs: &[SummaryInput],
        ctx: &SummaryContext<'_>,
    ) -> Result<SummaryOutput> {
        Ok(self.summarise_with_usage(inputs, ctx).await?.output)
    }

    async fn summarise_with_usage(
        &self,
        inputs: &[SummaryInput],
        ctx: &SummaryContext<'_>,
    ) -> Result<SummaryCall> {
        // Reuse the canonical fold prompt (honours the flavoured-tree `ask`).
        let prepared = match prepare_summary_prompt(inputs, ctx, None) {
            Some(p) => p,
            None => return Ok(SummaryCall::default()),
        };
        let before = self.usage();
        let text = self
            .chat(
                &prepared.system,
                &prepared.user,
                0.2,
                Some(prepared.effective_budget),
                false,
            )
            .await?;
        let after = self.usage();
        let output = finish_provider_summary(&text, ctx.token_budget);
        Ok(SummaryCall {
            output,
            input_tokens: after.prompt_tokens.saturating_sub(before.prompt_tokens),
            output_tokens: after
                .completion_tokens
                .saturating_sub(before.completion_tokens),
            charged_amount_usd: Some(after.cost_usd - before.cost_usd),
        })
    }
}

#[async_trait]
impl EmbeddingBackend for OpenRouterProvider {
    fn name(&self) -> &str {
        "openrouter"
    }

    fn model_id(&self) -> &str {
        &self.cfg.embed_model
    }

    fn dimensions(&self) -> usize {
        self.cfg.embed_dims
    }

    fn signature(&self) -> String {
        format_embedding_signature(
            EmbeddingBackend::name(self),
            &self.cfg.embed_model,
            self.cfg.embed_dims,
        )
    }

    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let body = json!({
            "model": self.cfg.embed_model,
            "input": texts,
        });
        let resp = self.post_json("embeddings", body).await?;
        if let Some(usage) = resp.get("usage") {
            self.record_usage(usage);
        }
        let data = resp
            .get("data")
            .and_then(|d| d.as_array())
            .ok_or_else(|| anyhow!("OpenRouter embeddings response missing data[]"))?;
        // The API preserves input order, but honour `index` defensively.
        let mut out: Vec<Vec<f32>> = vec![Vec::new(); data.len()];
        for (i, item) in data.iter().enumerate() {
            let idx = item.get("index").and_then(|v| v.as_u64()).unwrap_or(i as u64) as usize;
            let vec: Vec<f32> = item
                .get("embedding")
                .and_then(|e| e.as_array())
                .ok_or_else(|| anyhow!("OpenRouter embedding item missing embedding[]"))?
                .iter()
                .map(|v| v.as_f64().unwrap_or(0.0) as f32)
                .collect();
            if idx < out.len() {
                out[idx] = vec;
            }
        }
        if out.iter().any(|v| v.is_empty()) {
            return Err(anyhow!("OpenRouter embeddings returned fewer vectors than inputs"));
        }
        Ok(out)
    }
}

#[cfg(test)]
#[path = "openrouter_tests.rs"]
mod tests;
