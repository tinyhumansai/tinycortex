//! Tests for the deterministic fold (`fallback_summary` / `ConcatSummariser`).

use super::*;
use chrono::Utc;

fn sample_input(id: &str, content: &str) -> SummaryInput {
    let ts = Utc::now();
    SummaryInput {
        id: id.to_string(),
        content: content.to_string(),
        token_count: approx_token_count(content),
        entities: Vec::new(),
        topics: Vec::new(),
        time_range_start: ts,
        time_range_end: ts,
        score: 0.5,
    }
}

#[test]
fn fallback_concatenates_with_provenance_prefix() {
    let inputs = vec![sample_input("a", "hello"), sample_input("b", "world")];
    let out = fallback_summary(&inputs, 10_000);
    assert!(out.content.contains("hello"));
    assert!(out.content.contains("world"));
    assert!(out.content.contains("— "));
    assert!(out.entities.is_empty());
}

#[test]
fn fallback_truncates_at_budget() {
    let inputs = vec![sample_input("a", &"x".repeat(1000))];
    let out = fallback_summary(&inputs, 5);
    assert!(out.token_count <= 6);
}

#[test]
fn fallback_skips_blank_inputs() {
    let inputs = vec![sample_input("a", "   "), sample_input("b", "kept")];
    let out = fallback_summary(&inputs, 10_000);
    assert!(out.content.contains("kept"));
    assert_eq!(out.content.matches("— ").count(), 1);
}

#[tokio::test]
async fn concat_summariser_matches_fallback() {
    let inputs = vec![sample_input("a", "first"), sample_input("b", "second")];
    let ctx = SummaryContext {
        tree_id: "tree:test",
        tree_kind: TreeKind::Source,
        target_level: 1,
        token_budget: 10_000,
        input_token_budget: 50_000,
        overhead_reserve_tokens: 2_048,
        ask: None,
    };
    let out = ConcatSummariser::new()
        .summarise(&inputs, &ctx)
        .await
        .unwrap();
    assert!(out.content.contains("first"));
    assert!(out.content.contains("second"));
    assert_eq!(out.content, fallback_summary(&inputs, 10_000).content);
}

#[test]
fn provider_prompt_is_priority_ordered_language_aware_and_budgeted() {
    let mut low = sample_input("low", "low priority");
    low.score = 0.1;
    let mut high = sample_input("high", "high priority");
    high.score = 0.9;
    let context = SummaryContext {
        tree_id: "tree:test",
        tree_kind: TreeKind::Source,
        target_level: 1,
        token_budget: 9_000,
        input_token_budget: 50_000,
        overhead_reserve_tokens: 2_048,
        ask: None,
    };
    let prompt = prepare_summary_prompt(&[low, high], &context, Some("French")).unwrap();
    assert!(prompt.user.starts_with("[high]"));
    assert!(prompt.system.contains("Write the summary in French"));
    assert_eq!(prompt.effective_budget, 9_000);
    assert!(prepare_summary_prompt(&[], &context, None).is_none());
}
