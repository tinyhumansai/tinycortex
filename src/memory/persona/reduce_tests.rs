//! End-to-end map→reduce→compile tests using a mock chat provider and the
//! deterministic `ConcatSummariser` (the offline, zero-cost path §6.10).

use super::*;
use async_trait::async_trait;
use chrono::Utc;
use tempfile::TempDir;

use crate::memory::config::MemoryConfig;
use crate::memory::persona::compile::{compile_pack, PackInputs};
use crate::memory::persona::distill::digest_session;
use crate::memory::persona::readers::RawSession;
use crate::memory::persona::types::{
    EvidenceSource, EvidenceTier, PersonaEvidence, PersonaSourceKind,
};
use crate::memory::score::extract::{ChatPrompt, ChatProvider};
use crate::memory::tree::summarise::ConcatSummariser;

struct MockChat(String);

#[async_trait]
impl ChatProvider for MockChat {
    fn name(&self) -> &str {
        "mock"
    }
    async fn chat_for_json(&self, _p: &ChatPrompt) -> anyhow::Result<String> {
        Ok(self.0.clone())
    }
}

fn cfg() -> (TempDir, MemoryConfig) {
    let tmp = TempDir::new().unwrap();
    let cfg = MemoryConfig::new(tmp.path());
    (tmp, cfg)
}

fn session(scope: &str) -> RawSession {
    let src = EvidenceSource::new(PersonaSourceKind::ClaudeCode).with_scope(scope);
    let mut s = RawSession::new(src.clone());
    s.push(PersonaEvidence::new(
        src,
        Utc::now(),
        EvidenceTier::T2,
        "commit small and often",
        vec![],
    ));
    s
}

#[tokio::test]
async fn full_map_reduce_compile_offline() {
    let (_tmp, config) = cfg();
    let summariser = ConcatSummariser::new();
    let asks = FacetAsks::default();
    let mut state = ReduceState::default();

    // Mock digest: two facets of observations.
    let provider = MockChat(
        r#"{"observations":[
            {"facet":"workflow","observation":"Commits small and often","quote":"commit small and often","tier":"t2"},
            {"facet":"coding_style","observation":"Insists on regression tests","quote":"add a test","tier":"t1"}
        ]}"#
        .into(),
    );

    // Two sessions from two different scopes → cross-project strength.
    for scope in ["projA", "projB"] {
        let digest = digest_session(&provider, &session(scope)).await;
        fold_digest(&config, &digest, &asks, &summariser, &mut state)
            .await
            .unwrap();
    }

    // Fold a verbatim T0 directive too.
    let src = EvidenceSource::new(PersonaSourceKind::InstructionFile).with_scope("global");
    let directive = PersonaEvidence::new(
        src,
        Utc::now(),
        EvidenceTier::T0,
        "Always branch before writing code.",
        vec![PersonaFacet::Directives],
    );
    fold_directives(&config, std::slice::from_ref(&directive), &asks, &summariser, &mut state)
        .await
        .unwrap();

    // Seal + compile the facet trees.
    let bodies = seal_and_collect(&config, &asks, &summariser)
        .await
        .unwrap();
    assert!(bodies.contains_key(&PersonaFacet::Workflow));
    assert!(bodies.contains_key(&PersonaFacet::CodingStyle));
    assert!(bodies.contains_key(&PersonaFacet::Directives));

    // Compile the pack.
    let mut inputs = PackInputs::new("me@example.com");
    inputs.facet_bodies = bodies;
    inputs.counts = state.counts.clone();
    inputs.scopes = state.scopes.iter().map(|(k, v)| (*k, v.len())).collect();
    let pack = compile_pack(&inputs);

    assert!(pack.contains("# Persona: me@example.com"));
    assert!(pack.contains("## Workflow"));
    assert!(pack.contains("## Directives"));
    assert!(pack.contains("commit small and often") || pack.contains("Commits small"));
    // Workflow was seen in two projects.
    assert_eq!(state.scopes[&PersonaFacet::Workflow].len(), 2);
    assert_eq!(state.counts[&PersonaFacet::Workflow], 2);
    // The verbatim directive survives into the pack.
    assert!(pack.contains("Always branch before writing code."));
}

#[tokio::test]
async fn empty_digests_produce_empty_bodies() {
    let (_tmp, config) = cfg();
    let summariser = ConcatSummariser::new();
    let asks = FacetAsks::default();
    let bodies = seal_and_collect(&config, &asks, &summariser)
        .await
        .unwrap();
    assert!(bodies.is_empty(), "no leaves folded → no bodies");
}

#[test]
fn strip_frontmatter_removes_yaml_block() {
    let md = "---\nkind: flavoured_root\nask: \"x\"\n---\nThe body here.\n";
    assert_eq!(strip_frontmatter(md), "The body here.");
    assert_eq!(strip_frontmatter("no frontmatter"), "no frontmatter");
}
