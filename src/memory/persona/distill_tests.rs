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
    let digest = digest_session(&provider, &session).await.unwrap();
    assert_eq!(digest.observations.len(), 2);
    assert_eq!(digest.observations[0].facet, PersonaFacet::Workflow);
    assert_eq!(digest.observations[1].facet, PersonaFacet::CodingStyle);
    assert_eq!(digest.observations[1].tier, EvidenceTier::T1);
}

#[tokio::test]
async fn tolerates_prose_wrapped_json() {
    let body = "Sure! Here is the JSON:\n```json\n{\"observations\":[{\"facet\":\"stack\",\"observation\":\"Uses Rust\",\"quote\":\"cargo\",\"tier\":\"t2\"}]}\n```";
    let provider = MockChat {
        body: Ok(body.into()),
    };
    let session = session_with(&[("cargo test", EvidenceTier::T2)]);
    let digest = digest_session(&provider, &session).await.unwrap();
    assert_eq!(digest.observations.len(), 1);
    assert_eq!(digest.observations[0].facet, PersonaFacet::Stack);
}

#[tokio::test]
async fn hard_and_unparseable_are_non_committable_errors() {
    let session = session_with(&[("x", EvidenceTier::T2)]);

    // A hard provider failure surfaces as Err (so the caller won't commit the
    // cursor and the session is retried next run).
    let failing = MockChat {
        body: Err("402 requires more credits".into()),
    };
    assert!(digest_session(&failing, &session).await.is_err());

    // A received-but-unparseable response is ALSO an Err now (B3): it must NOT be
    // treated as a committable empty digest, because "no JSON at all" is
    // indistinguishable from a response that was cut off before any observation
    // could be read. Committing it would mark the window done and drop it.
    let garbage = MockChat {
        body: Ok("not json at all".into()),
    };
    assert!(digest_session(&garbage, &session).await.is_err());
}

/// A response truncated mid-array (a well-formed prefix with the closing `]`
/// missing — exactly what a hit output-token cap produces) must surface as an
/// Err, never as a committable empty digest. This is the data-loss case B3
/// fixes: the model *did* produce observations, so silently dropping the window
/// and committing its cursor loses them forever.
#[tokio::test]
async fn truncated_json_array_is_a_non_committable_error() {
    // A well-formed prefix: two complete observation objects, but the array's
    // closing `]` and the outer `}` never arrive (the model hit its output cap).
    // Both parse attempts in `parse_digest` fail — the raw string, and the
    // first-`{`..last-`}` slice, which still lacks the `]`/`}` — so this reports
    // "EOF while parsing a list" rather than degrading to an empty digest.
    let truncated = r#"{"observations":[
        {"facet":"workflow","observation":"Commits small and often","quote":"commit small","tier":"t2"},
        {"facet":"coding_style","observation":"Insists on regression tests","quote":"add a test","tier":"t1"}"#;
    let provider = MockChat {
        body: Ok(truncated.into()),
    };
    let session = session_with(&[("x", EvidenceTier::T2)]);
    let result = digest_session(&provider, &session).await;
    assert!(
        result.is_err(),
        "a truncated observation array must be a retryable Err, got: {result:?}"
    );
}

#[tokio::test]
async fn empty_session_yields_empty_digest() {
    let provider = MockChat {
        body: Ok("{\"observations\":[]}".into()),
    };
    let session = RawSession::new(EvidenceSource::new(PersonaSourceKind::Codex));
    assert!(digest_session(&provider, &session)
        .await
        .unwrap()
        .is_empty());
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
    let digest = digest_session(&provider, &session).await.unwrap();
    assert_eq!(digest.observations.len(), 1);
    assert_eq!(digest.observations[0].observation, "Prefers Postgres");
}
