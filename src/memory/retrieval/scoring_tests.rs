//! Unit tests for the hybrid scoring signals and composition. The weight-
//! profile composition mirrors OpenHuman's `memory_search::scoring` tests.

use super::*;
use crate::memory::config::WeightProfile;

#[test]
fn profiles_sum_to_one() {
    for profile in [
        WeightProfile::BALANCED,
        WeightProfile::SEMANTIC,
        WeightProfile::LEXICAL,
        WeightProfile::GRAPH_FIRST,
    ] {
        let sum = profile.graph + profile.vector + profile.keyword + profile.freshness;
        assert!(
            (sum - 1.0).abs() < 0.01,
            "profile weights should sum to ~1.0, got {sum}"
        );
    }
}

#[test]
fn compose_applies_weights() {
    let bd = hybrid_score(&WeightProfile::SEMANTIC, 0.5, 1.0, 0.5, 0.0);
    let expected = 0.15 * 0.5 + 0.65 * 1.0 + 0.20 * 0.5 + 0.0 * 0.0;
    assert!((bd.final_score - expected).abs() < 1e-9);
    // Component signals are echoed into the breakdown for explainability.
    assert_eq!(bd.graph_relevance, 0.5);
    assert_eq!(bd.vector_similarity, 1.0);
    assert_eq!(bd.keyword_relevance, 0.5);
    assert_eq!(bd.episodic_relevance, 0.0);
}

#[test]
fn keyword_relevance_full_and_partial_overlap() {
    assert_eq!(
        keyword_relevance("phoenix migration", "the phoenix migration ships"),
        1.0
    );
    // One of two query tokens present → 0.5.
    assert_eq!(
        keyword_relevance("phoenix launch", "the phoenix migration"),
        0.5
    );
    assert_eq!(keyword_relevance("phoenix", "totally unrelated"), 0.0);
}

#[test]
fn keyword_relevance_empty_query_or_content_is_zero() {
    assert_eq!(keyword_relevance("", "anything"), 0.0);
    assert_eq!(keyword_relevance("query", ""), 0.0);
}

#[test]
fn keyword_relevance_is_case_insensitive() {
    assert_eq!(keyword_relevance("Phoenix", "the PHOENIX project"), 1.0);
}

#[test]
fn freshness_now_is_one_and_decays() {
    let now = 1_700_000_000_000;
    assert_eq!(freshness(now, now, 7.0), 1.0);
    // Exactly one half-life old → 0.5.
    let one_half_life = now - (7 * 86_400_000);
    assert!((freshness(one_half_life, now, 7.0) - 0.5).abs() < 1e-6);
    // Older scores lower than a half-life.
    let two_half_lives = now - (14 * 86_400_000);
    assert!(freshness(two_half_lives, now, 7.0) < 0.5);
}

#[test]
fn freshness_future_timestamp_clamps_to_one() {
    let now = 1_700_000_000_000;
    assert_eq!(freshness(now + 86_400_000, now, 7.0), 1.0);
}

#[test]
fn freshness_zero_half_life_disables_decay() {
    let now = 1_700_000_000_000;
    assert_eq!(freshness(now - 100 * 86_400_000, now, 0.0), 1.0);
}
