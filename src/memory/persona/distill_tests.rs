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
    let digest = digest_session(&provider, &session).await;
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
    let digest = digest_session(&provider, &session).await;
    assert_eq!(digest.observations.len(), 1);
    assert_eq!(digest.observations[0].facet, PersonaFacet::Stack);
}

#[tokio::test]
async fn soft_falls_back_on_error_and_bad_json() {
    let session = session_with(&[("x", EvidenceTier::T2)]);

    let failing = MockChat {
        body: Err("402 requires more credits".into()),
    };
    assert!(digest_session(&failing, &session).await.is_empty());

    let garbage = MockChat {
        body: Ok("not json at all".into()),
    };
    assert!(digest_session(&garbage, &session).await.is_empty());
}

#[tokio::test]
async fn empty_session_yields_empty_digest() {
    let provider = MockChat {
        body: Ok("{\"observations\":[]}".into()),
    };
    let session = RawSession::new(EvidenceSource::new(PersonaSourceKind::Codex));
    assert!(digest_session(&provider, &session).await.is_empty());
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
    let digest = digest_session(&provider, &session).await;
    assert_eq!(digest.observations.len(), 1);
    assert_eq!(digest.observations[0].observation, "Prefers Postgres");
}
