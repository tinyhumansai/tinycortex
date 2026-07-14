//! Unit + integration tests for the algorithmic persona retriever.

use super::*;
use chrono::Utc;
use tempfile::TempDir;

use crate::memory::config::MemoryConfig;
use crate::memory::persona::reduce::{fold_digest, FacetAsks, ReduceState};
use crate::memory::persona::types::{
    DigestObservation, EvidenceSource, PersonaSourceKind, SessionDigest,
};
use crate::memory::tree::summarise::ConcatSummariser;

fn doc(facet: PersonaFacet, line: &str) -> ObsDoc {
    parse_observation(facet, line, Utc::now()).expect("parses")
}

#[test]
fn parses_observation_with_quote_and_tier() {
    let d = doc(
        PersonaFacet::Workflow,
        "- Always branch before writing code (\"never commit to main\") [t0]",
    );
    assert_eq!(d.tier, EvidenceTier::T0);
    assert_eq!(d.text, "Always branch before writing code");
    assert_eq!(d.quote.as_deref(), Some("never commit to main"));
    // Quote terms are searchable too.
    assert!(d.term_freqs.contains_key("branch"));
    assert!(d.term_freqs.contains_key("commit"));
}

#[test]
fn parses_observation_without_quote() {
    let d = doc(
        PersonaFacet::CodingStyle,
        "- Keep modules under 500 lines [t2]",
    );
    assert_eq!(d.tier, EvidenceTier::T2);
    assert_eq!(d.text, "Keep modules under 500 lines");
    assert!(d.quote.is_none());
}

#[test]
fn missing_or_garbled_tier_defaults_to_t3() {
    let d = doc(PersonaFacet::Stack, "- Prefers Rust for systems work");
    assert_eq!(d.tier, EvidenceTier::T3);
    let d2 = doc(PersonaFacet::Stack, "- Prefers Rust [nonsense]");
    assert_eq!(d2.tier, EvidenceTier::T3);
    // A non-tier trailing bracket stays part of the text.
    assert!(d2.text.contains("[nonsense]"));
}

#[test]
fn non_bullet_lines_are_skipped() {
    assert!(parse_observation(PersonaFacet::Stack, "plain text", Utc::now()).is_none());
    assert!(parse_observation(PersonaFacet::Stack, "- ", Utc::now()).is_none());
    assert!(parse_observation(PersonaFacet::Stack, "", Utc::now()).is_none());
}

#[test]
fn tokenizer_drops_stopwords_and_shorts() {
    let toks = tokenize("The agent SHOULD use Rust_2021 and cargo fmt");
    assert!(toks.contains(&"agent".to_string()));
    assert!(toks.contains(&"rust_2021".to_string()));
    assert!(toks.contains(&"cargo".to_string()));
    assert!(toks.contains(&"fmt".to_string()));
    assert!(!toks.contains(&"the".to_string()));
    assert!(!toks.contains(&"should".to_string()));
    assert!(!toks.contains(&"and".to_string()));
}

#[test]
fn search_ranks_lexically_relevant_first() {
    let docs = vec![
        doc(
            PersonaFacet::CodingStyle,
            "- Write focused unit tests beside each module [t2]",
        ),
        doc(
            PersonaFacet::Stack,
            "- Reach for tokio and async Rust by default [t2]",
        ),
        doc(
            PersonaFacet::Workflow,
            "- Commit small and often with clear messages [t2]",
        ),
    ];
    let r = PersonaRetriever::from_docs(docs);
    let hits = r.search("how should I structure my tests", None, 3);
    assert!(!hits.is_empty());
    assert!(hits[0].text.contains("unit tests"));
}

#[test]
fn facet_filter_restricts_results() {
    let docs = vec![
        doc(
            PersonaFacet::CodingStyle,
            "- Prefer explicit error handling with Result [t2]",
        ),
        doc(
            PersonaFacet::Stack,
            "- Prefer Rust and Result-based error types [t2]",
        ),
    ];
    let r = PersonaRetriever::from_docs(docs);
    let hits = r.search("error handling", Some(PersonaFacet::Stack), 5);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].facet, PersonaFacet::Stack);
}

#[test]
fn higher_tier_wins_at_equal_relevance() {
    // Identical text, different tiers → the higher-confidence rule ranks first.
    let docs = vec![
        doc(
            PersonaFacet::Workflow,
            "- Use git worktrees for parallel agents [t3]",
        ),
        doc(
            PersonaFacet::Workflow,
            "- Use git worktrees for parallel agents [t0]",
        ),
    ];
    let r = PersonaRetriever::from_docs(docs);
    let hits = r.search("git worktrees parallel", None, 2);
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].tier, EvidenceTier::T0);
    assert!(hits[0].score >= hits[1].score);
}

#[test]
fn empty_query_or_corpus_yields_nothing() {
    let r = PersonaRetriever::from_docs(vec![doc(PersonaFacet::Stack, "- Prefer Rust [t2]")]);
    assert!(r.search("", None, 5).is_empty());
    assert!(r.search("rust", None, 0).is_empty());
    let empty = PersonaRetriever::from_docs(vec![]);
    assert!(empty.is_empty());
    assert!(empty.search("rust", None, 5).is_empty());
}

#[tokio::test]
async fn loads_from_a_real_persona_workspace() {
    let tmp = TempDir::new().unwrap();
    let config = MemoryConfig::new(tmp.path());
    let summariser = ConcatSummariser::new();
    let mut state = ReduceState::default();

    let digest = SessionDigest {
        source: EvidenceSource::new(PersonaSourceKind::ClaudeCode).with_scope("tinycortex"),
        observations: vec![
            DigestObservation {
                facet: PersonaFacet::CodingStyle,
                observation: "Keep modules under 500 lines and split before that".to_string(),
                quote: "Avoid letting any source file grow beyond 500 lines".to_string(),
                tier: EvidenceTier::T0,
            },
            DigestObservation {
                facet: PersonaFacet::Workflow,
                observation: "Branch before writing code; never commit to main".to_string(),
                quote: String::new(),
                tier: EvidenceTier::T0,
            },
        ],
    };

    fold_digest(
        &config,
        &digest,
        &FacetAsks::default(),
        &summariser,
        &mut state,
    )
    .await
    .unwrap();

    let retriever = PersonaRetriever::load(&config).unwrap();
    assert_eq!(retriever.len(), 2);

    let counts = retriever.facet_counts();
    assert_eq!(counts.get(&PersonaFacet::CodingStyle).copied(), Some(1));
    assert_eq!(counts.get(&PersonaFacet::Workflow).copied(), Some(1));

    let hits = retriever.search("how many lines per module file", None, 3);
    assert!(hits[0].text.contains("500 lines"));
    assert_eq!(hits[0].facet, PersonaFacet::CodingStyle);
    assert_eq!(hits[0].tier, EvidenceTier::T0);
}
