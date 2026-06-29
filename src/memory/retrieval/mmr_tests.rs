//! MMR selection tests, ported from OpenHuman's `memory_search::vector::mmr`.

use super::*;

fn make_vec(vals: &[f32]) -> Vec<f32> {
    vals.to_vec()
}

#[test]
fn empty_candidates_returns_empty() {
    let query = make_vec(&[1.0, 0.0, 0.0]);
    let result = mmr_select(&query, &[], 5, 0.7);
    assert!(result.is_empty());
}

#[test]
fn single_candidate() {
    let query = make_vec(&[1.0, 0.0, 0.0]);
    let emb = make_vec(&[1.0, 0.0, 0.0]);
    let candidates = vec![MmrCandidate {
        index: 0,
        embedding: &emb,
        relevance: 0.95,
    }];
    let result = mmr_select(&query, &candidates, 5, 0.7);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].index, 0);
}

#[test]
fn diversity_selects_distinct_vectors() {
    let query = make_vec(&[1.0, 0.0, 0.0]);
    let dup1 = make_vec(&[0.99, 0.01, 0.0]);
    let dup2 = make_vec(&[0.98, 0.02, 0.0]);
    let dup3 = make_vec(&[0.97, 0.03, 0.0]);
    let distinct1 = make_vec(&[0.0, 1.0, 0.0]);
    let distinct2 = make_vec(&[0.0, 0.0, 1.0]);

    let candidates = vec![
        MmrCandidate {
            index: 0,
            embedding: &dup1,
            relevance: 0.99,
        },
        MmrCandidate {
            index: 1,
            embedding: &dup2,
            relevance: 0.98,
        },
        MmrCandidate {
            index: 2,
            embedding: &dup3,
            relevance: 0.97,
        },
        MmrCandidate {
            index: 3,
            embedding: &distinct1,
            relevance: 0.50,
        },
        MmrCandidate {
            index: 4,
            embedding: &distinct2,
            relevance: 0.45,
        },
    ];

    let result = mmr_select(&query, &candidates, 3, 0.5);
    assert_eq!(result.len(), 3);
    let selected_indices: Vec<usize> = result.iter().map(|r| r.index).collect();
    let dup_count = selected_indices.iter().filter(|&&i| i <= 2).count();
    assert!(dup_count <= 2, "MMR should diversify away from duplicates");
    let distinct_count = selected_indices.iter().filter(|&&i| i >= 3).count();
    assert!(
        distinct_count >= 1,
        "MMR should select at least one distinct vector"
    );
}

#[test]
fn lambda_one_is_pure_relevance() {
    let query = make_vec(&[1.0, 0.0, 0.0]);
    let emb1 = make_vec(&[0.99, 0.01, 0.0]);
    let emb2 = make_vec(&[0.0, 1.0, 0.0]);
    let candidates = vec![
        MmrCandidate {
            index: 0,
            embedding: &emb1,
            relevance: 0.99,
        },
        MmrCandidate {
            index: 1,
            embedding: &emb2,
            relevance: 0.50,
        },
    ];
    let result = mmr_select(&query, &candidates, 2, 1.0);
    assert_eq!(result[0].index, 0);
    assert_eq!(result[1].index, 1);
}

#[test]
fn limit_caps_output() {
    let query = make_vec(&[1.0, 0.0]);
    let embs: Vec<Vec<f32>> = (0..10)
        .map(|i| make_vec(&[1.0 - i as f32 * 0.1, i as f32 * 0.1]))
        .collect();
    let candidates: Vec<MmrCandidate> = embs
        .iter()
        .enumerate()
        .map(|(i, e)| MmrCandidate {
            index: i,
            embedding: e,
            relevance: 1.0 - i as f64 * 0.1,
        })
        .collect();
    let result = mmr_select(&query, &candidates, 3, 0.7);
    assert_eq!(result.len(), 3);
}
