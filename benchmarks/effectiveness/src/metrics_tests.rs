//! Unit tests for the pure ranking metrics.

use super::*;

fn set(ids: &[&str]) -> HashSet<String> {
    ids.iter().map(|s| s.to_string()).collect()
}

fn ranked(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|s| s.to_string()).collect()
}

#[test]
fn recall_counts_relevant_within_cutoff() {
    let r = ranked(&["a", "b", "c", "d"]);
    let rel = set(&["b", "d", "z"]); // z is unreachable
    assert_eq!(recall_at_k(&r, &rel, 2), 1.0 / 3.0); // only b in top-2
    assert_eq!(recall_at_k(&r, &rel, 4), 2.0 / 3.0); // b and d in top-4
}

#[test]
fn recall_empty_relevant_is_zero() {
    assert_eq!(recall_at_k(&ranked(&["a"]), &set(&[]), 5), 0.0);
}

#[test]
fn precision_divides_by_k() {
    let r = ranked(&["a", "b", "c", "d"]);
    let rel = set(&["a", "c"]);
    assert_eq!(precision_at_k(&r, &rel, 4), 0.5); // 2 of 4
    assert_eq!(precision_at_k(&r, &rel, 0), 0.0);
}

#[test]
fn hit_is_binary() {
    let r = ranked(&["a", "b", "c"]);
    assert_eq!(hit_at_k(&r, &set(&["c"]), 3), 1.0);
    assert_eq!(hit_at_k(&r, &set(&["c"]), 2), 0.0);
}

#[test]
fn reciprocal_rank_uses_first_hit() {
    let r = ranked(&["a", "b", "c"]);
    assert_eq!(reciprocal_rank(&r, &set(&["b"])), 0.5); // rank 2
    assert_eq!(reciprocal_rank(&r, &set(&["a", "c"])), 1.0); // rank 1
    assert_eq!(reciprocal_rank(&r, &set(&["z"])), 0.0); // no hit
}

#[test]
fn ndcg_is_one_for_ideal_ordering() {
    let r = ranked(&["a", "b", "c", "d"]);
    let rel = set(&["a", "b"]); // both at the top => perfect
    assert!((ndcg_at_k(&r, &rel, 4) - 1.0).abs() < 1e-9);
}

#[test]
fn ndcg_penalizes_late_hits() {
    let ideal = ndcg_at_k(&ranked(&["a", "x", "y"]), &set(&["a"]), 3);
    let late = ndcg_at_k(&ranked(&["x", "y", "a"]), &set(&["a"]), 3);
    assert!((ideal - 1.0).abs() < 1e-9);
    assert!(late < ideal);
    assert!(late > 0.0);
}

#[test]
fn ndcg_empty_relevant_is_zero() {
    assert_eq!(ndcg_at_k(&ranked(&["a", "b"]), &set(&[]), 5), 0.0);
}
