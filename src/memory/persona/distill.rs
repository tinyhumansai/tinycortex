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
use super::types::{DigestObservation, EvidenceTier, PersonaFacet, SessionDigest};
use crate::memory::score::extract::{ChatPrompt, ChatProvider};
use crate::memory::store::safety::sanitize_text;

/// Max characters of evidence sent in a single digest call. Larger sessions are
/// split into windows and digested part-by-part.
const WINDOW_CHARS: usize = 12_000;
/// Output-token cap for a digest response.
///
/// A window holds up to [`WINDOW_CHARS`] of evidence, and a dense window of
/// corrections/directives (e.g. Codex sessions) can distil into *many* facet
/// observations, each carrying an `observation` string and a supporting `quote`.
/// The response is a single JSON object, but its size scales with the observation
/// count — not with the "one small object" a 4 K cap assumed. At 4 K the model
/// ran out of output mid-array and emitted a well-formed prefix with no closing
/// `]`, which `parse_digest` then rejected (`EOF while parsing a list`, observed
/// at column ~15–19 K); B3 stops that from silently dropping the window, and this
/// larger cap stops it from happening in the first place. 16 K comfortably covers
/// the observed truncations while still bounding a runaway generation. Coupled to
/// [`WINDOW_CHARS`]: raising the input window raises the observations a window can
/// yield, so the two move together.
const DIGEST_MAX_OUTPUT_TOKENS: u32 = 16_384;

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

/// Digest one session into a [`SessionDigest`] via the chat provider.
///
/// Distinguishes two outcomes so the pipeline can checkpoint correctly:
/// - **Non-committable failure** → returns `Err`. The caller must NOT commit the
///   session's cursor, so the evidence is re-attempted on the next run. This
///   covers both a provider/transport error (a `chat_for_json` error — budget
///   exhausted, 401/403, transport) *and* a response we could not parse (most
///   often a **truncated** JSON array — the model hit its output-token cap
///   mid-list, so the observations it *did* find would be lost forever if we
///   committed). See [`DigestError`].
/// - **Genuinely empty digest** (a valid call whose response parsed to zero
///   observations, e.g. `{"observations":[]}`) → returns `Ok` with an empty
///   digest. Re-running would reproduce it, so the cursor IS committed.
pub async fn digest_session(
    provider: &dyn ChatProvider,
    session: &RawSession,
) -> Result<SessionDigest> {
    if session.is_empty() {
        return Ok(SessionDigest::empty(session.source.clone()));
    }
    let mut observations: Vec<DigestObservation> = Vec::new();
    for window in windows(session) {
        // A hard provider failure OR an unparseable/truncated window bubbles up
        // as `Err` (the whole session is retried next run, nothing committed);
        // only a cleanly-parsed empty window is tolerated as `Ok(vec![])`.
        let obs = digest_window(provider, session, &window).await?;
        observations.extend(obs);
    }
    Ok(SessionDigest {
        source: session.source.clone(),
        observations,
    })
}

/// Why a window could not be digested into committable observations.
///
/// Both variants are **retryable** — the caller must not commit the session's
/// cursor for either, so the window is re-attempted on the next run. They are
/// distinguished only for logging/telemetry clarity. A genuinely-empty result is
/// *not* an error (it is `Ok(vec![])`); this type exists so a truncated or
/// otherwise unparseable response can no longer masquerade as "empty" and be
/// silently committed (the data-loss bug this fixes).
#[derive(Debug, thiserror::Error)]
enum DigestError {
    /// The provider call itself failed (budget/auth/transport). The response was
    /// never received.
    #[error("digest provider call failed: {0:#}")]
    Provider(#[source] anyhow::Error),
    /// A response was received but could not be parsed — typically a JSON array
    /// truncated at the output-token cap (a well-formed prefix with no closing
    /// `]`). Committing would drop the observations the model *did* produce.
    #[error("digest response unparseable (likely truncated at the output cap): {0:#}")]
    Unparseable(#[source] anyhow::Error),
}

/// One window → observations.
///
/// Returns `Err` for **both** a `chat_for_json` failure and an
/// unparseable/truncated response — both are non-committable so the window is
/// retried next run (a truncated array must NOT be treated as "empty and done",
/// or the observations already generated are lost). A cleanly-parsed response
/// with zero usable observations returns `Ok(vec![])`, which the caller commits
/// because re-running reproduces it.
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
    let raw = provider
        .chat_for_json(&prompt)
        .await
        .map_err(DigestError::Provider)?;
    let parsed: RawDigest = parse_digest(&raw).map_err(|e| {
        log::warn!(
            "[persona] digest parse failed for {} ({}); NOT committing cursor so \
             the window is retried next run: {e:#}",
            session.source.kind.as_str(),
            session.source.session_id.as_deref().unwrap_or("?")
        );
        DigestError::Unparseable(e)
    })?;
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
    Err(anyhow::anyhow!(
        "digest response was not JSON: {}",
        raw.chars().take(120).collect::<String>()
    ))
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
