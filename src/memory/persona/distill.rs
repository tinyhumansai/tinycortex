//! Distillation map step (doc 06 §6.5): one `chat_for_json` call per extracted
//! session (or commit batch) turning a [`RawSession`] of redacted evidence into
//! a [`SessionDigest`] of per-facet, prescriptive observations.
//!
//! Follows the `LlmEntityExtractor` pattern: a strict-JSON instruction with the
//! schema in the prompt, and a **soft fallback** — any failure (transport,
//! malformed JSON, empty) skips the session by returning an empty digest rather
//! than aborting the run. Oversized sessions are windowed and digested in parts,
//! and the observations are concatenated.

use anyhow::Result;
use serde::Deserialize;

use super::readers::RawSession;
use super::types::{
    DigestObservation, EvidenceTier, PersonaFacet, SessionDigest,
};
use crate::memory::score::extract::{ChatPrompt, ChatProvider};
use crate::memory::store::safety::sanitize_text;

/// Max characters of evidence sent in a single digest call. Larger sessions are
/// split into windows and digested part-by-part.
const WINDOW_CHARS: usize = 12_000;
/// Output-token cap for a digest response (one small JSON object).
const DIGEST_MAX_OUTPUT_TOKENS: u32 = 4_096;

/// The strict-JSON system prompt: schema + extraction contract.
fn system_prompt() -> String {
    let facets = PersonaFacet::ALL
        .iter()
        .map(|f| f.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "You analyse a person's own messages to their AI coding agents (and their \
git commit style). Your job is to distil a durable profile of THE PERSON so another \
agent can mimic them — never describe the agent or the task.\n\n\
Extract prescriptive observations across these facets: {facets}.\n\
- communication: tone, verbosity, directness, phrasing quirks, how they give feedback.\n\
- coding_style: naming, structure, comments, error handling, testing, size discipline.\n\
- stack: languages, frameworks, libraries, architectural choices they favour.\n\
- workflow: branching/commit granularity, plan-first vs dive-in, PR/review habits, parallelism.\n\
- environment: editors, agent harnesses, CLIs, package managers, OS.\n\
- directives: explicit standing rules they state.\n\
- anti_preferences: things they dislike, correct, revert, or forbid.\n\n\
Each observation must be a concrete rule an agent should follow to act like this \
person, backed by a short supporting quote drawn from the evidence, plus a \
confidence tier:\n\
- t0: an explicit written rule.\n\
- t1: a correction/interrupt (the person stopped the agent and redirected it).\n\
- t2: prompt phrasing, command habits, commit-message style.\n\
- t3: inferred from accepted outcomes (weak; corroborates only).\n\n\
Only emit well-supported observations; omit weak guesses. Respond with a JSON \
object of exactly this shape and nothing else:\n\
{{\"observations\":[{{\"facet\":\"coding_style\",\"observation\":\"...\",\"quote\":\"...\",\"tier\":\"t2\"}}]}}"
    )
}

/// Build the user prompt for one window of evidence.
fn user_prompt(session: &RawSession, window: &str) -> String {
    let provenance = format!(
        "source={} scope={}",
        session.source.kind.as_str(),
        session.source.scope.as_deref().unwrap_or("(unknown)")
    );
    format!("Provenance: {provenance}\n\nEvidence (each line is one unit):\n{window}\n\nReturn JSON only.")
}

/// Split a session's evidence into windows of at most [`WINDOW_CHARS`] chars,
/// each a newline-joined block of the evidence excerpts with their tiers.
fn windows(session: &RawSession) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ev in &session.evidence {
        let line = format!("[{}] {}\n", ev.tier.as_str(), ev.excerpt());
        if !cur.is_empty() && cur.len() + line.len() > WINDOW_CHARS {
            out.push(std::mem::take(&mut cur));
        }
        // A single oversized unit is truncated to the window.
        if line.len() > WINDOW_CHARS {
            cur.push_str(&line.chars().take(WINDOW_CHARS).collect::<String>());
            out.push(std::mem::take(&mut cur));
        } else {
            cur.push_str(&line);
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}

/// Digest one session into a [`SessionDigest`] via the chat provider. Soft-falls
/// back to an empty digest on any failure so the pipeline never aborts.
pub async fn digest_session(
    provider: &dyn ChatProvider,
    session: &RawSession,
) -> SessionDigest {
    if session.is_empty() {
        return SessionDigest::empty(session.source.clone());
    }
    let mut observations: Vec<DigestObservation> = Vec::new();
    for window in windows(session) {
        match digest_window(provider, session, &window).await {
            Ok(mut obs) => observations.append(&mut obs),
            Err(e) => {
                log::warn!(
                    "[persona] digest failed for {} ({}): {e:#}",
                    session.source.kind.as_str(),
                    session.source.session_id.as_deref().unwrap_or("?")
                );
                // soft fallback: skip this window
            }
        }
    }
    SessionDigest {
        source: session.source.clone(),
        observations,
    }
}

/// One window → observations. Errors bubble so the caller can soft-fall back.
async fn digest_window(
    provider: &dyn ChatProvider,
    session: &RawSession,
    window: &str,
) -> Result<Vec<DigestObservation>> {
    let prompt = ChatPrompt {
        system: system_prompt(),
        user: user_prompt(session, window),
        temperature: 0.0,
        kind: "persona::digest",
        max_tokens: Some(DIGEST_MAX_OUTPUT_TOKENS),
    };
    let raw = provider.chat_for_json(&prompt).await?;
    let parsed: RawDigest = parse_digest(&raw)?;
    Ok(parsed
        .observations
        .into_iter()
        .filter_map(RawObservation::into_observation)
        .collect())
}

/// Parse a digest response, tolerating models that wrap the JSON in prose or
/// code fences by extracting the first `{...}` object.
fn parse_digest(raw: &str) -> Result<RawDigest> {
    if let Ok(d) = serde_json::from_str::<RawDigest>(raw) {
        return Ok(d);
    }
    let start = raw.find('{');
    let end = raw.rfind('}');
    if let (Some(s), Some(e)) = (start, end) {
        if e > s {
            return Ok(serde_json::from_str::<RawDigest>(&raw[s..=e])?);
        }
    }
    Err(anyhow::anyhow!("digest response was not JSON: {}", raw.chars().take(120).collect::<String>()))
}

#[derive(Debug, Deserialize)]
struct RawDigest {
    #[serde(default)]
    observations: Vec<RawObservation>,
}

#[derive(Debug, Deserialize)]
struct RawObservation {
    #[serde(default)]
    facet: String,
    #[serde(default)]
    observation: String,
    #[serde(default)]
    quote: String,
    #[serde(default)]
    tier: String,
}

impl RawObservation {
    /// Validate and normalise one raw observation, dropping unusable ones.
    fn into_observation(self) -> Option<DigestObservation> {
        let facet = PersonaFacet::parse_loose(&self.facet)?;
        let observation = self.observation.trim().to_string();
        if observation.len() < 3 {
            return None;
        }
        let tier = EvidenceTier::parse_loose(&self.tier).unwrap_or(EvidenceTier::T3);
        // Defensive: redact the quote in case the model echoed raw evidence.
        let quote = sanitize_text(self.quote.trim()).value;
        Some(DigestObservation {
            facet,
            observation: sanitize_text(&observation).value,
            quote,
            tier,
        })
    }
}

#[cfg(test)]
#[path = "distill_tests.rs"]
mod tests;
