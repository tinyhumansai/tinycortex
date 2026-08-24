//! Tests for the digest map step (mock chat provider).

use super::*;
use async_trait::async_trait;
use chrono::Utc;

use crate::memory::persona::readers::RawSession;
use crate::memory::persona::types::{EvidenceSource, EvidenceTier, PersonaSourceKind};

/// A chat provider that returns a canned body (or errors) for every call.
struct MockChat {
    body: Result<String, String>,
}

#[async_trait]
impl ChatProvider for MockChat {
    fn name(&self) -> &str {
        "mock"
    }
    async fn chat_for_json(&self, _prompt: &ChatPrompt) -> anyhow::Result<String> {
        self.body.clone().map_err(|e| anyhow::anyhow!(e))
    }
}

/// A chat provider that truncates the JSON array (drops the closing `]`/`}`) when
/// the prompt's evidence window is large, and returns a clean single-observation
/// object once the window is small enough — modelling a real output-token cap
/// that only trips on dense windows and clears once recovery re-splits them.
struct SizeAwareChat {
    /// Prompt-length threshold (chars) above which the response truncates.
    threshold: usize,
}

#[async_trait]
impl ChatProvider for SizeAwareChat {
    fn name(&self) -> &str {
        "size-aware"
    }
    async fn chat_for_json(&self, prompt: &ChatPrompt) -> anyhow::Result<String> {
        if prompt.user.chars().count() > self.threshold {
            // Well-formed prefix, no closing `]`/`}` — a hit output cap.
            Ok(r#"{"observations":[
                {"facet":"workflow","observation":"Commits small and often","quote":"commit","tier":"t2"}"#
                .into())
        } else {
            Ok(r#"{"observations":[
                {"facet":"stack","observation":"Uses Rust everywhere","quote":"cargo","tier":"t2"}
            ]}"#
            .into())
        }
    }
}

/// A chat provider that truncates the response iff the prompt contains `marker`,
/// and returns a clean single-observation object otherwise — modelling a window
/// that stays unparseable no matter how small it is re-split (the poison window).
struct MarkerChat {
    marker: &'static str,
}

#[async_trait]
impl ChatProvider for MarkerChat {
    fn name(&self) -> &str {
        "marker"
    }
    async fn chat_for_json(&self, prompt: &ChatPrompt) -> anyhow::Result<String> {
        if prompt.user.contains(self.marker) {
            Ok(r#"{"observations":[
                {"facet":"workflow","observation":"Commits small and often","quote":"commit","tier":"t2"}"#
                .into())
        } else {
            Ok(r#"{"observations":[
                {"facet":"stack","observation":"Uses Rust everywhere","quote":"cargo","tier":"t2"}
            ]}"#
            .into())
        }
    }
}

fn session_with(excerpts: &[(&str, EvidenceTier)]) -> RawSession {
    let src = EvidenceSource::new(PersonaSourceKind::ClaudeCode).with_scope("demo");
    let mut s = RawSession::new(src.clone());
    for (text, tier) in excerpts {
        s.push(crate::memory::persona::types::PersonaEvidence::new(
            src.clone(),
            Utc::now(),
            *tier,
            text,
            vec![],
        ));
    }
    s
}

/// A session whose evidence spans `total_chars` (one line), forcing a single
/// large window that a size-aware provider will truncate until it is re-split.
fn big_session(total_chars: usize) -> RawSession {
    // Repeated ASCII with no PII patterns, so `sanitize_text` leaves it intact
    // and the window length is predictable.
    let filler = "abcdefghij".repeat(total_chars / 10 + 1);
    session_with(&[(&filler[..total_chars], EvidenceTier::T2)])
}

/// Like [`big_session`] but `char_count` *multibyte* characters (byte length ≫
/// char count), so the recovery splitter is exercised on char — not byte —
/// boundaries.
fn big_multibyte_session(char_count: usize) -> RawSession {
    let filler: String = "日本語コード".chars().cycle().take(char_count).collect();
    session_with(&[(filler.as_str(), EvidenceTier::T2)])
}

#[tokio::test]
async fn parses_observations_from_json() {
    let body = r#"{"observations":[
        {"facet":"workflow","observation":"Commits small and often","quote":"commit small","tier":"t2"},
        {"facet":"coding-style","observation":"Wants regression tests","quote":"add a test","tier":"t1"}
    ]}"#;
    let provider = MockChat {
        body: Ok(body.into()),
    };
    let session = session_with(&[("commit small and often", EvidenceTier::T2)]);
    let outcome = digest_session(&provider, &session, &CallBudget::unlimited())
        .await
        .unwrap();
    assert_eq!(outcome.windows_lost, 0);
    let obs = outcome.digest.observations;
    assert_eq!(obs.len(), 2);
    assert_eq!(obs[0].facet, PersonaFacet::Workflow);
    assert_eq!(obs[1].facet, PersonaFacet::CodingStyle);
    assert_eq!(obs[1].tier, EvidenceTier::T1);
}

#[tokio::test]
async fn tolerates_prose_wrapped_json() {
    let body = "Sure! Here is the JSON:\n```json\n{\"observations\":[{\"facet\":\"stack\",\"observation\":\"Uses Rust\",\"quote\":\"cargo\",\"tier\":\"t2\"}]}\n```";
    let provider = MockChat {
        body: Ok(body.into()),
    };
    let session = session_with(&[("cargo test", EvidenceTier::T2)]);
    let outcome = digest_session(&provider, &session, &CallBudget::unlimited())
        .await
        .unwrap();
    assert_eq!(outcome.windows_lost, 0);
    assert_eq!(outcome.digest.observations.len(), 1);
    assert_eq!(outcome.digest.observations[0].facet, PersonaFacet::Stack);
}

/// A hard provider failure (transport/budget/auth) is transient: it surfaces as
/// `Err` so the caller won't commit the cursor and retries the whole session.
#[tokio::test]
async fn provider_failure_is_a_non_committable_error() {
    let session = session_with(&[("x", EvidenceTier::T2)]);
    let failing = MockChat {
        body: Err("402 requires more credits".into()),
    };
    assert!(digest_session(&failing, &session, &CallBudget::unlimited())
        .await
        .is_err());
}

/// A truncated window that a smaller call *can* parse must be **recovered**, not
/// lost: recovery re-splits the window, the halves fit under the cap, and every
/// observation is kept with nothing counted as lost. This is the data-loss the
/// PR exists to fix, proven end-to-end through the re-split path.
#[tokio::test]
async fn truncated_window_is_recovered_by_resplitting() {
    // One ~8 000-char window. The full window exceeds the threshold (truncates),
    // but each half (~4 000) is under it and parses cleanly.
    let session = big_session(8_000);
    let provider = SizeAwareChat { threshold: 5_000 };
    let outcome = digest_session(&provider, &session, &CallBudget::unlimited())
        .await
        .unwrap();
    assert_eq!(
        outcome.windows_lost, 0,
        "a re-splittable truncation must recover, not drop"
    );
    assert!(
        !outcome.digest.observations.is_empty(),
        "recovered halves must yield observations"
    );
}

/// Truncation recovery must draw from the run's `max_llm_calls` budget: a window
/// that would re-split into several calls cannot exceed it. With a budget of one
/// call, the first (truncated) attempt spends it and the re-split can't proceed,
/// so the session is a non-committable `BudgetExhausted` checkpoint (not a silent
/// partial). Guards against a recovery tree blowing the configured cap.
#[tokio::test]
async fn recovery_respects_the_call_budget() {
    let session = big_session(8_000);
    let provider = SizeAwareChat { threshold: 5_000 };
    let budget = CallBudget::new(1);
    let err = digest_session(&provider, &session, &budget)
        .await
        .expect_err("a one-call budget can't complete a re-splitting recovery");
    assert!(
        is_budget_exhausted(&err),
        "budget exhaustion must be classifiable as a checkpoint, got: {err:#}"
    );
    assert_eq!(budget.remaining(), 0, "the one call was spent");
}

/// A provider that truncates *every* call, even the smallest sub-window, must not
/// retry forever: recovery bottoms out at the minimum size, drops the window, and
/// tallies it in `windows_lost` — returning `Ok` so the cursor can commit and the
/// queue keeps moving (the permanent-starvation guard).
#[tokio::test]
async fn permanently_unparseable_window_terminates_and_is_counted() {
    let truncated = r#"{"observations":[
        {"facet":"workflow","observation":"Commits small and often","quote":"commit small","tier":"t2"},
        {"facet":"coding_style","observation":"Insists on regression tests","quote":"add a test","tier":"t1"}"#;
    let provider = MockChat {
        body: Ok(truncated.into()),
    };
    let session = session_with(&[("x", EvidenceTier::T2)]);
    let outcome = digest_session(&provider, &session, &CallBudget::unlimited())
        .await
        .unwrap();
    assert_eq!(
        outcome.windows_lost, 1,
        "an irreducible truncation is dropped-and-counted, not looped forever"
    );
    assert!(outcome.digest.observations.is_empty());
}

/// The bounded re-splitting *descent*: a window sized to `MIN_WINDOW_CHARS <<
/// MAX_RESPLIT_DEPTH` (12k) splits binarily the full 12k → 6k → 3k → 1.5k chain,
/// digesting a complete recovery tree and dropping every leaf. Pinning the call
/// count to the exact tree size — `sum(2^i for i in 0..=depth) = 2^(depth+1) - 1`
/// — detects a fan-out regression, not merely a hang. (The test above covers the
/// immediate below-floor drop; this exercises the `MAX_RESPLIT_DEPTH` path.)
#[tokio::test]
async fn large_permanently_unparseable_window_terminates_within_bounded_calls() {
    let truncated = r#"{"observations":[
        {"facet":"workflow","observation":"Commits small and often","quote":"commit small","tier":"t2"}"#;
    let provider = MockChat {
        body: Ok(truncated.into()),
    };
    let session = big_session(MIN_WINDOW_CHARS << MAX_RESPLIT_DEPTH); // 1500 * 8 = 12_000
    let budget = CallBudget::new(1_000);
    let outcome = digest_session(&provider, &session, &budget).await.unwrap();
    assert!(outcome.digest.observations.is_empty());
    // Leaves are the `2^depth` pieces at the floor (8 here), each dropped-and-counted.
    assert_eq!(outcome.windows_lost, 1 << MAX_RESPLIT_DEPTH);
    let calls = 1_000 - budget.remaining();
    let expected_calls = (1usize << (MAX_RESPLIT_DEPTH + 1)) - 1; // 1+2+4+8 = 15
    assert_eq!(
        calls, expected_calls,
        "recovery must spend exactly the bounded tree ({expected_calls}), spent {calls}"
    );
}

/// Recovery re-splits on *character* boundaries, so a multibyte window that never
/// parses walks the same descent without a byte-slice panic and drops the same
/// leaf count as its ASCII twin.
#[tokio::test]
async fn recovery_splits_multibyte_windows_without_panicking() {
    let truncated = r#"{"observations":[
        {"facet":"workflow","observation":"Commits small and often","quote":"commit small","tier":"t2"}"#;
    let provider = MockChat {
        body: Ok(truncated.into()),
    };
    let session = big_multibyte_session(MIN_WINDOW_CHARS << MAX_RESPLIT_DEPTH);
    let budget = CallBudget::new(1_000);
    let outcome = digest_session(&provider, &session, &budget).await.unwrap();
    assert!(outcome.digest.observations.is_empty());
    assert_eq!(
        outcome.windows_lost,
        1 << MAX_RESPLIT_DEPTH,
        "multibyte leaves drop-and-count exactly like ASCII, with no panic"
    );
}

/// A multi-window session with one permanently-bad window keeps the observations
/// from the clean window (no all-or-nothing abort) and counts only the bad one.
#[tokio::test]
async fn one_bad_window_does_not_discard_the_clean_siblings() {
    // A near-full first line (~11 800 chars) flushes as its own clean window, then
    // a short marker line (~300 chars, below the re-split floor) forms a second
    // window that truncates and cannot be split further → dropped-and-counted.
    let clean = "abcdefghij".repeat(1_200); // ~12 000 chars available
    let poison = format!("POISONWINDOW {}", "z".repeat(280));
    let session = session_with(&[
        (&clean[..11_800], EvidenceTier::T2),
        (poison.as_str(), EvidenceTier::T1),
    ]);
    let provider = MarkerChat {
        marker: "POISONWINDOW",
    };
    let outcome = digest_session(&provider, &session, &CallBudget::unlimited())
        .await
        .unwrap();
    assert_eq!(
        outcome.windows_lost, 1,
        "only the irreducible poison window is lost"
    );
    assert!(
        !outcome.digest.observations.is_empty(),
        "the clean sibling window's observations are retained"
    );
}

#[tokio::test]
async fn empty_session_yields_empty_digest() {
    let provider = MockChat {
        body: Ok("{\"observations\":[]}".into()),
    };
    let session = RawSession::new(EvidenceSource::new(PersonaSourceKind::Codex));
    let outcome = digest_session(&provider, &session, &CallBudget::unlimited())
        .await
        .unwrap();
    assert!(outcome.digest.is_empty());
    assert_eq!(outcome.windows_lost, 0);
}

/// A cleanly-parsed empty digest for a *non-empty* session is committable: it is
/// `Ok` with no observations and nothing lost (the guard against over-correcting
/// the truncation fix into a permanent retry of genuinely-empty windows).
#[tokio::test]
async fn clean_empty_observations_commit_without_loss() {
    let provider = MockChat {
        body: Ok("{\"observations\":[]}".into()),
    };
    let session = session_with(&[("just chatting, no rules here", EvidenceTier::T3)]);
    let outcome = digest_session(&provider, &session, &CallBudget::unlimited())
        .await
        .unwrap();
    assert!(outcome.digest.is_empty());
    assert_eq!(
        outcome.windows_lost, 0,
        "a clean empty response is not a loss and must commit"
    );
}

#[tokio::test]
async fn drops_unusable_observations() {
    // Unknown facet + too-short observation are both dropped.
    let body = r#"{"observations":[
        {"facet":"bogus","observation":"whatever","tier":"t2"},
        {"facet":"stack","observation":"x","tier":"t2"},
        {"facet":"stack","observation":"Prefers Postgres","quote":"","tier":"t3"}
    ]}"#;
    let provider = MockChat {
        body: Ok(body.into()),
    };
    let session = session_with(&[("db", EvidenceTier::T2)]);
    let outcome = digest_session(&provider, &session, &CallBudget::unlimited())
        .await
        .unwrap();
    assert_eq!(outcome.digest.observations.len(), 1);
    assert_eq!(
        outcome.digest.observations[0].observation,
        "Prefers Postgres"
    );
}
