//! LLM-based entity + importance extractor (trait-abstracted backend).
//!
//! Builds a `(system, user)` prompt asking for NER + an importance rating in
//! one structured-JSON response, hands the prompt to a [`ChatProvider`], and
//! parses the result into [`ExtractedEntities`].
//!
//! ## Trait abstraction (TinyCortex)
//!
//! Unlike OpenHuman, TinyCortex never depends on a concrete chat backend. The
//! extractor holds an `Arc<dyn ChatProvider>` — defined here — so tests inject
//! a mock and production wires a real provider from an adapter crate. The crate
//! itself never calls a real model.
//!
//! ## Span recovery
//!
//! LLMs are unreliable about character offsets. We re-find each returned entity
//! surface in the source text via `str::find` to recover spans. Entities whose
//! surface form can't be located are dropped (this catches model hallucinations).
//!
//! ## Soft fallback
//!
//! If the chat call fails (provider unavailable, malformed JSON, …), we return
//! [`ExtractedEntities::default()`]. The
//! [`super::CompositeExtractor`] already tolerates errors from individual
//! extractors; ingestion never blocks on LLM availability.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;

use super::types::{EntityKind, ExtractedEntities, ExtractedEntity, ExtractedTopic};
use super::EntityExtractor;

#[path = "llm_prompt.rs"]
mod prompt;
use prompt::build_system_prompt;

/// Output-token cap requested for an extraction call (`max_tokens` on the
/// wire). An extraction response is one small structured-JSON object, so a few
/// thousand tokens is generous. The cap keeps credit-metered providers from
/// reserving the model's *entire* output window during balance pre-flight.
const EXTRACTION_MAX_OUTPUT_TOKENS: u32 = 8192;

// ── Chat provider abstraction ────────────────────────────────────────────

/// A single structured-JSON chat request.
///
/// Mirrors the minimal shape the extractor needs from a chat backend. Hosts
/// adapt their own provider stack to [`ChatProvider`]; this struct is the
/// stable wire contract between the extractor and that adapter.
#[derive(Clone, Debug)]
pub struct ChatPrompt {
    /// System prompt — the JSON schema + extraction instructions.
    pub system: String,
    /// User prompt — the text to extract from.
    pub user: String,
    /// Sampling temperature. Extraction uses `0.0` for determinism.
    pub temperature: f32,
    /// Stable workload tag for routing / metrics.
    pub kind: &'static str,
    /// Optional output-token cap.
    pub max_tokens: Option<u32>,
}

/// Pluggable chat backend. Implementations turn a [`ChatPrompt`] into a raw
/// JSON string response (or an error). The crate ships **no** real
/// implementation — tests inject a mock and hosts wire their own.
#[async_trait]
pub trait ChatProvider: Send + Sync {
    /// Stable short name for logs and diagnostics.
    fn name(&self) -> &str;

    /// Run one chat completion and return the raw response body (expected to
    /// be a JSON object string). Errors surface transport / provider failures.
    async fn chat_for_json(&self, prompt: &ChatPrompt) -> anyhow::Result<String>;
}

// ── Configuration ────────────────────────────────────────────────────────

/// Configuration for [`LlmEntityExtractor`].
#[derive(Clone, Debug)]
pub struct LlmExtractorConfig {
    /// Model identifier the chat provider should target. Stored for diagnostic
    /// logging only — actual model selection happens inside the [`ChatProvider`].
    pub model: String,
    /// Which entity kinds the LLM is allowed to emit. Anything outside this set
    /// is mapped to [`EntityKind::Misc`] or dropped depending on `strict_kinds`.
    pub allowed_kinds: Vec<EntityKind>,
    /// If true, drop entities whose declared kind isn't in `allowed_kinds`
    /// instead of falling back to [`EntityKind::Misc`].
    pub strict_kinds: bool,
    /// If true, the system prompt asks the model to also emit a `topics` array
    /// (free-form theme labels) and the response parser populates
    /// [`ExtractedEntities::topics`]. Default `false`.
    pub emit_topics: bool,
    /// Optional configured output language for natural-language values such as
    /// `importance_reason` and topic labels. JSON field names and enum values
    /// remain stable.
    pub output_language: Option<String>,
}

impl Default for LlmExtractorConfig {
    fn default() -> Self {
        Self {
            model: "qwen2.5:0.5b".to_string(),
            allowed_kinds: vec![
                EntityKind::Person,
                EntityKind::Organization,
                EntityKind::Location,
                EntityKind::Event,
                EntityKind::Product,
                EntityKind::Datetime,
                EntityKind::Technology,
                EntityKind::Artifact,
                EntityKind::Quantity,
            ],
            strict_kinds: false,
            emit_topics: false,
            output_language: None,
        }
    }
}

// ── Extractor ────────────────────────────────────────────────────────────

/// LLM-backed entity + importance extractor.
///
/// Holds an `Arc<dyn ChatProvider>`; the provider abstraction lets a single
/// workspace choose a backend at runtime. Tests mock the provider to assert
/// prompt / parse behaviour without a real model.
pub struct LlmEntityExtractor {
    cfg: LlmExtractorConfig,
    provider: Arc<dyn ChatProvider>,
}

impl LlmEntityExtractor {
    /// Build the extractor with the supplied chat provider. Infallible — the
    /// caller is responsible for provider construction.
    pub fn new(cfg: LlmExtractorConfig, provider: Arc<dyn ChatProvider>) -> Self {
        Self { cfg, provider }
    }

    /// Build the chat prompt sent to the provider for `text`.
    fn build_prompt(&self, text: &str) -> ChatPrompt {
        ChatPrompt {
            system: build_system_prompt(self.cfg.emit_topics, self.cfg.output_language.as_deref()),
            user: format!("Text:\n{text}\n\nReturn JSON only."),
            temperature: 0.0,
            kind: "memory_tree::extract",
            max_tokens: Some(EXTRACTION_MAX_OUTPUT_TOKENS),
        }
    }
}

#[async_trait]
impl EntityExtractor for LlmEntityExtractor {
    fn name(&self) -> &'static str {
        "llm"
    }

    async fn extract(&self, text: &str) -> anyhow::Result<ExtractedEntities> {
        // Soft-fallback contract: every failure path (transport, HTTP status,
        // JSON parse) is logged as a warn and returns an empty
        // `ExtractedEntities` rather than `Err`. This makes the extractor safe
        // to call from any context.
        //
        // Transport failures get bounded retries before falling back to empty.
        // Non-transport failures (permanent client errors, malformed JSON) fall
        // back immediately because retrying the same input yields the same
        // result.
        const MAX_ATTEMPTS: u32 = 3;

        for _ in 0..MAX_ATTEMPTS {
            match self.try_extract(text).await {
                AttemptOutcome::Done(extracted) => return Ok(extracted),
                // Permanent client error — retrying reproduces it. Soft-fall back
                // to an empty extraction immediately.
                AttemptOutcome::Permanent => return Ok(ExtractedEntities::default()),
                // Transient failure — loop and try again until attempts run out.
                AttemptOutcome::Retryable => continue,
            }
        }

        // Every attempt hit a transient failure; degrade to empty.
        Ok(ExtractedEntities::default())
    }
}

/// Outcome of a single [`LlmEntityExtractor::try_extract`] attempt.
enum AttemptOutcome {
    /// The call completed (provider returned content). Includes the
    /// "malformed wrong-shape JSON → empty" case.
    Done(ExtractedEntities),
    /// Transport / transient failure. Retrying may help.
    Retryable,
    /// Permanent, non-retryable provider failure (4xx client error: out of
    /// credits, rejected key, model gone). Retrying reproduces the same error.
    Permanent,
}

impl LlmEntityExtractor {
    /// Internal: one attempt at calling the chat provider.
    async fn try_extract(&self, text: &str) -> AttemptOutcome {
        let prompt = self.build_prompt(text);

        let raw = match self.provider.chat_for_json(&prompt).await {
            Ok(v) => v,
            Err(e) => {
                // A non-retryable client error must not be retried. Only
                // genuine transport/transient failures earn another attempt.
                if is_non_retryable(&e) {
                    return AttemptOutcome::Permanent;
                }
                return AttemptOutcome::Retryable;
            }
        };

        let parsed: LlmExtractionOutput = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(e) if e.is_eof() => {
                // Truncated mid-JSON: the response stream closed before the
                // closing brace. Unlike a wrong-shape body, this is transient.
                return AttemptOutcome::Retryable;
            }
            Err(_e) => {
                // Non-EOF parse error: a complete but wrong-shape body. Retrying
                // would yield the same response, so degrade to an empty result.
                return AttemptOutcome::Done(ExtractedEntities::default());
            }
        };

        AttemptOutcome::Done(parsed.into_extracted_entities(text, &self.cfg))
    }
}

/// Classify a provider error as non-retryable (permanent client error) vs
/// transient. Permanent errors (out of credits / quota, rejected key) reproduce
/// on retry, so the retry loop must short-circuit them. Pure string heuristic —
/// the crate has no provider stack to inspect.
fn is_non_retryable(err: &anyhow::Error) -> bool {
    let lower = format!("{err:#}").to_lowercase();
    lower.contains("402")
        || lower.contains("payment required")
        || lower.contains("requires more credits")
        || lower.contains("insufficient")
        || lower.contains("monthly_request_count")
        || lower.contains("monthly request")
        || lower.contains("quota")
        || lower.contains("401")
        || lower.contains("403")
        || lower.contains("unauthorized")
        || lower.contains("forbidden")
        || lower.contains("invalid api key")
        || lower.contains("incorrect api key")
}

// ── LLM JSON output ──────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct LlmExtractionOutput {
    #[serde(default)]
    entities: Vec<LlmEntity>,
    /// Free-form theme labels — populated only when the extractor is configured
    /// with `emit_topics = true`. Always tolerant of absence.
    #[serde(default)]
    topics: Vec<String>,
    #[serde(default)]
    importance: Option<f32>,
    #[serde(default)]
    importance_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LlmEntity {
    kind: String,
    text: String,
}

impl LlmExtractionOutput {
    fn into_extracted_entities(
        self,
        source_text: &str,
        cfg: &LlmExtractorConfig,
    ) -> ExtractedEntities {
        let mut entities = Vec::with_capacity(self.entities.len());

        // Per-surface search cursor (char offset). When the LLM returns the
        // same surface text twice, resume searching AFTER the previous
        // occurrence so each emitted entity points at a distinct span.
        use std::collections::HashMap;
        let mut cursors: HashMap<String, (usize /*byte*/, u32 /*char*/)> = HashMap::new();

        for raw in self.entities {
            let surface = raw.text.trim();
            if surface.is_empty() {
                continue;
            }

            let kind = match parse_kind(&raw.kind) {
                Some(k) => {
                    if cfg.allowed_kinds.contains(&k) {
                        k
                    } else if cfg.strict_kinds {
                        continue;
                    } else {
                        EntityKind::Misc
                    }
                }
                None => {
                    if cfg.strict_kinds {
                        continue;
                    }
                    EntityKind::Misc
                }
            };

            // Recover spans by string search, advancing the cursor for this
            // surface so repeated mentions get distinct spans. If the model
            // hallucinated a surface (or we've exhausted its occurrences),
            // drop the entity.
            let (byte_from, char_from) = cursors.get(surface).copied().unwrap_or((0, 0));
            let (span_start, span_end, byte_after) =
                match find_char_span_from(source_text, surface, byte_from, char_from) {
                    Some(s) => s,
                    None => {
                        continue;
                    }
                };
            cursors.insert(surface.to_string(), (byte_after, span_end));

            entities.push(ExtractedEntity {
                kind,
                text: surface.to_string(),
                span_start,
                span_end,
                score: 0.85, // LLM-derived; lower confidence than regex
            });
        }

        let llm_importance = self.importance.map(|v| v.clamp(0.0, 1.0));

        // Topics: only populated when the caller enabled `emit_topics`.
        let topics = self
            .topics
            .into_iter()
            .filter_map(|raw| {
                let label = raw.trim().to_string();
                if label.is_empty() {
                    None
                } else {
                    Some(ExtractedTopic { label, score: 0.85 })
                }
            })
            .collect();

        ExtractedEntities {
            entities,
            topics,
            llm_importance,
            llm_importance_reason: self.importance_reason,
        }
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────

fn parse_kind(s: &str) -> Option<EntityKind> {
    match s.trim().to_lowercase().as_str() {
        "person" | "people" => Some(EntityKind::Person),
        "organization" | "organisation" | "org" => Some(EntityKind::Organization),
        "location" | "place" | "loc" => Some(EntityKind::Location),
        "event" => Some(EntityKind::Event),
        "product" => Some(EntityKind::Product),
        "datetime" | "date" | "time" | "timestamp" => Some(EntityKind::Datetime),
        "technology" | "tech" | "tool" | "framework" | "library" | "language" | "service" => {
            Some(EntityKind::Technology)
        }
        "artifact" | "reference" | "ref" | "pr" | "ticket" | "file" | "commit" => {
            Some(EntityKind::Artifact)
        }
        "quantity" | "amount" | "metric" | "number" | "money" => Some(EntityKind::Quantity),
        "misc" | "miscellaneous" | "other" => Some(EntityKind::Misc),
        _ => None,
    }
}

/// Find `needle` in `haystack` and return its `(char_start, char_end)`.
///
/// Single-occurrence convenience wrapper around [`find_char_span_from`]; kept
/// for the span-recovery test suite and external reuse.
#[allow(dead_code)]
fn find_char_span(haystack: &str, needle: &str) -> Option<(u32, u32)> {
    find_char_span_from(haystack, needle, 0, 0).map(|(s, e, _)| (s, e))
}

/// Find `needle` in `haystack` starting from `byte_from` and return
/// `(char_start, char_end, byte_after_needle)`.
///
/// `char_from` must correspond to `byte_from` in the same `haystack`. The
/// caller maintains this invariant (cheap: it's the return from the previous
/// call).
fn find_char_span_from(
    haystack: &str,
    needle: &str,
    byte_from: usize,
    char_from: u32,
) -> Option<(u32, u32, usize)> {
    if needle.is_empty() || byte_from > haystack.len() {
        return None;
    }
    // Guard against `byte_from` landing inside a multi-byte UTF-8 sequence.
    if !haystack.is_char_boundary(byte_from) {
        return None;
    }
    let rel = haystack[byte_from..].find(needle)?;
    let byte_start = byte_from + rel;
    let byte_end = byte_start + needle.len();
    let char_start = char_from + haystack[byte_from..byte_start].chars().count() as u32;
    let char_end = char_start + needle.chars().count() as u32;
    Some((char_start, char_end, byte_end))
}

#[allow(dead_code)]
fn truncate_for_log(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let truncated: String = s.chars().take(max_chars).collect();
    format!("{truncated}…")
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
#[path = "llm_tests.rs"]
mod tests;
