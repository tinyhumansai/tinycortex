//! Distillation map step (doc 06 §6.5): one `chat_for_json` call per extracted
//! session (or commit batch) turning a [`RawSession`] of redacted evidence into
//! a [`SessionDigest`] of per-facet, prescriptive observations.
//!
//! Follows the `LlmEntityExtractor` pattern: a strict-JSON instruction with the
//! schema in the prompt. Failure handling is deliberately **not** a silent
//! soft-fallback (that was the data-loss bug this module fixes):
//! - a provider/transport failure returns `Err` so the caller leaves the whole
//!   session non-committable and retries it next run;
//! - a truncated/unparseable response — the output-token cap cutting the JSON
//!   array short — is first *recovered* in-process by re-splitting the window
//!   into smaller pieces whose responses fit under the cap, and only a piece that
//!   still won't parse at the minimum size is dropped-and-counted (surfaced in
//!   the run report, never retried forever — a deterministically-bad window must
//!   not starve the queue);
//! - a cleanly-parsed empty response commits normally.
//!
//! Oversized sessions are windowed and digested part-by-part; each window is
//! digested independently, so one bad window never discards the clean siblings
//! that already digested — their observations are accumulated and kept.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

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
/// Smallest sub-window truncation-recovery ([`digest_window_recovering`]) will
/// produce. Below this a response that still won't parse is treated as genuinely
/// broken (not merely output-capped) and dropped-with-a-count rather than split
/// further — the floor that guarantees recovery terminates.
const MIN_WINDOW_CHARS: usize = 1_500;
/// Max times recovery halves a truncated window before giving up on a sub-window.
/// `12_000 → 6_000 → 3_000 → 1_500` reaches [`MIN_WINDOW_CHARS`], an ~8× cut in
/// the per-call output that overran the cap — deep enough to recover real
/// truncations, bounded so a deterministically-bad window can't loop.
const MAX_RESPLIT_DEPTH: usize = 3;

/// A shared, concurrency-safe ceiling on provider calls for one run.
///
/// `digest_window` consumes one permit per `chat_for_json`, so **windowing and
/// truncation-recovery re-tries alike** count against the configured
/// `max_llm_calls`: a recovery tree (up to ~15 calls per truncated window across
/// many windows) can no longer overrun the run's call budget. Cheap to clone and
/// share across the concurrently-digested sessions — it wraps an
/// `Arc<AtomicUsize>` of the remaining calls.
#[derive(Clone)]
pub struct CallBudget(Arc<AtomicUsize>);

impl CallBudget {
    /// A budget of `max_calls` provider calls for the run.
    pub fn new(max_calls: usize) -> Self {
        Self(Arc::new(AtomicUsize::new(max_calls)))
    }

    /// A budget that never runs out — for callers/tests that don't gate calls.
    pub fn unlimited() -> Self {
        Self::new(usize::MAX)
    }

    /// Reserve one call, returning `false` once the budget is spent. Lock-free
    /// and correct under the concurrent `buffered` digest stream.
    fn try_acquire(&self) -> bool {
        self.0
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok()
    }

    /// Calls still available in this run.
    pub fn remaining(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
}

/// True when `err` (from [`digest_session`]) is the run's provider-call budget
/// being hit mid-session — a clean checkpoint to resume next run, not a failure.
pub fn is_budget_exhausted(err: &anyhow::Error) -> bool {
    err.downcast_ref::<DigestError>()
        .is_some_and(|e| matches!(e, DigestError::BudgetExhausted))
}

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

/// Outcome of digesting one session: the observations plus how many windows were
/// **dropped** because they stayed unparseable even after truncation-recovery
/// re-splitting.
///
/// A non-zero `windows_lost` still commits the session's cursor: the failure is
/// deterministic (temperature `0.0`), so retrying would only re-burn budget
/// without recovering anything, and holding the cursor would starve every newer
/// session behind it. The count is surfaced in the run report so the drop is
/// visible, never silent. Only a *provider* failure (transport/budget/auth) is a
/// non-committable `Err` from [`digest_session`] — that is transient and worth
/// retrying the whole session for.
#[derive(Debug, Clone)]
pub struct SessionOutcome {
    /// The observations distilled from every digested (or recovered) window.
    pub digest: SessionDigest,
    /// Windows dropped after recovery could not parse them (data intentionally
    /// skipped to keep the queue moving).
    pub windows_lost: usize,
}

/// Digest one session into a [`SessionOutcome`] via the chat provider.
///
/// Windows are digested independently and their observations accumulated, so a
/// bad window never discards the clean siblings that already digested. Outcomes:
/// - **Provider failure** (a `chat_for_json` error — budget exhausted, 401/403,
///   transport) → returns `Err`. The caller must NOT commit the session's cursor,
///   so the whole session is re-attempted next run. This is transient.
/// - **Truncated/unparseable window** → recovered in-process by re-splitting (see
///   the private `digest_window_recovering`); a piece that still won't parse at the
///   minimum size is dropped and tallied in [`SessionOutcome::windows_lost`]. The cursor
///   is still committed — the failure is deterministic, so retrying is pure waste.
/// - **Genuinely empty digest** (`{"observations":[]}`) → `Ok` with an empty
///   digest and `windows_lost = 0`. Re-running reproduces it, so the cursor commits.
/// - **Call budget exhausted** mid-session → returns `Err` (classify with
///   [`is_budget_exhausted`]); the session is left non-committable and resumes
///   next run. A clean checkpoint, not a failure.
pub async fn digest_session(
    provider: &dyn ChatProvider,
    session: &RawSession,
    budget: &CallBudget,
) -> Result<SessionOutcome> {
    if session.is_empty() {
        return Ok(SessionOutcome {
            digest: SessionDigest::empty(session.source.clone()),
            windows_lost: 0,
        });
    }
    let mut observations: Vec<DigestObservation> = Vec::new();
    let mut windows_lost = 0usize;
    for window in windows(session) {
        // Each window recovers from truncation on its own and never aborts its
        // siblings; a hard provider failure or an exhausted call budget bubbles
        // up as `Err` (the whole session is then retried next run, nothing
        // committed).
        let (obs, lost) = digest_window_recovering(provider, session, &window, budget)
            .await
            .map_err(anyhow::Error::new)?;
        observations.extend(obs);
        windows_lost += lost;
    }
    Ok(SessionOutcome {
        digest: SessionDigest {
            source: session.source.clone(),
            observations,
        },
        windows_lost,
    })
}

/// Why a window could not be digested into observations.
#[derive(Debug, thiserror::Error)]
enum DigestError {
    /// The provider call itself failed (budget/auth/transport). The response was
    /// never received — transient, so the caller retries the whole session.
    #[error("digest provider call failed: {0:#}")]
    Provider(#[source] anyhow::Error),
    /// A response was received but could not be parsed — typically a JSON array
    /// truncated at the output-token cap (a well-formed prefix with no closing
    /// `]`). Recoverable by re-splitting the window into smaller pieces.
    #[error("digest response unparseable (likely truncated at the output cap): {0:#}")]
    Unparseable(#[source] anyhow::Error),
    /// The run's provider-call budget (`max_llm_calls`) was spent before this
    /// window could be digested. The session is left non-committable so it
    /// resumes next run — a clean checkpoint, not a failure.
    #[error("digest call budget exhausted")]
    BudgetExhausted,
}

/// Digest one window, recovering from output-cap truncation by re-splitting.
///
/// The first attempt digests the whole window. If the response is unparseable —
/// the signature of a JSON array cut off at the output-token cap — the window is
/// halved and each half retried, which shrinks the per-call output below the cap
/// and *recovers* the observations instead of discarding them. Halving is bounded
/// by [`MAX_RESPLIT_DEPTH`] and [`MIN_WINDOW_CHARS`]; a sub-window still
/// unparseable at the floor is genuinely broken (not merely truncated), so it is
/// dropped and counted in the returned `lost` tally rather than retried forever —
/// that is what stops a single deterministically-bad window from starving the
/// queue on every run.
///
/// A provider/transport failure is *not* recovered here: it returns `Err` so the
/// caller leaves the whole session non-committable and retries it next run.
async fn digest_window_recovering(
    provider: &dyn ChatProvider,
    session: &RawSession,
    window: &str,
    budget: &CallBudget,
) -> Result<(Vec<DigestObservation>, usize), DigestError> {
    let mut observations = Vec::new();
    let mut lost = 0usize;
    // Work stack of (text, splits_remaining). Order does not matter — evidence
    // ids are content-addressed, so folding is insensitive to window order.
    let mut stack: Vec<(String, usize)> = vec![(window.to_string(), MAX_RESPLIT_DEPTH)];
    while let Some((piece, splits_remaining)) = stack.pop() {
        match digest_window(provider, session, &piece, budget).await {
            Ok(obs) => observations.extend(obs),
            // Transient / budget checkpoint — abort recovery and let the whole
            // session retry next run (nothing committed).
            Err(DigestError::Provider(e)) => return Err(DigestError::Provider(e)),
            Err(DigestError::BudgetExhausted) => return Err(DigestError::BudgetExhausted),
            Err(DigestError::Unparseable(e)) => {
                let target = piece.chars().count() / 2;
                let parts = if splits_remaining > 0 && target >= MIN_WINDOW_CHARS {
                    split_window(&piece, target)
                } else {
                    Vec::new()
                };
                if parts.len() > 1 {
                    for part in parts {
                        stack.push((part, splits_remaining - 1));
                    }
                } else {
                    // Irreducible and still unparseable: drop it, but loudly and
                    // counted — a deterministic failure must make progress, not
                    // hold the cursor and retry forever.
                    log::warn!(
                        "[persona] digest window unrecoverable after re-splitting for {} ({}); \
                         dropping {} chars and committing the rest so the queue is not starved: {e:#}",
                        session.source.kind.as_str(),
                        session.source.session_id.as_deref().unwrap_or("?"),
                        piece.chars().count(),
                    );
                    lost += 1;
                }
            }
        }
    }
    Ok((observations, lost))
}

/// One window → observations.
///
/// Returns [`DigestError::Provider`] for a `chat_for_json` failure and
/// [`DigestError::Unparseable`] for a truncated/unparseable response; a
/// cleanly-parsed response with zero usable observations returns `Ok(vec![])`.
async fn digest_window(
    provider: &dyn ChatProvider,
    session: &RawSession,
    window: &str,
    budget: &CallBudget,
) -> Result<Vec<DigestObservation>, DigestError> {
    // Reserve the call against the run budget *before* spending it, so windowing
    // and every recovery re-try count toward `max_llm_calls`.
    if !budget.try_acquire() {
        return Err(DigestError::BudgetExhausted);
    }
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
            "[persona] digest parse failed for {} ({}); attempting in-process \
             re-split recovery before giving up: {e:#}",
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

/// Split `text` into chunks of at most `target` **chars**, breaking on line
/// boundaries; a single line longer than `target` is hard-split on char
/// boundaries (UTF-8-safe). Used by truncation recovery to shrink a window whose
/// digest overran the output-token cap. Char counts throughout (not byte lengths)
/// so a multibyte-heavy window still halves to a genuinely smaller piece.
fn split_window(text: &str, target: usize) -> Vec<String> {
    let target = target.max(1);
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut cur_len = 0usize;
    for line in text.split_inclusive('\n') {
        let line_len = line.chars().count();
        if cur_len > 0 && cur_len + line_len > target {
            out.push(std::mem::take(&mut cur));
            cur_len = 0;
        }
        if line_len > target {
            // Oversized single line: hard-split on char boundaries.
            let mut chunk = String::new();
            let mut chunk_len = 0usize;
            for ch in line.chars() {
                if chunk_len >= target {
                    out.push(std::mem::take(&mut chunk));
                    chunk_len = 0;
                }
                chunk.push(ch);
                chunk_len += 1;
            }
            cur.push_str(&chunk);
            cur_len += chunk_len;
        } else {
            cur.push_str(line);
            cur_len += line_len;
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
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
