//! `fetch_leaves` tests, ported from OpenHuman's `memory_tree::retrieval::fetch`.

use super::*;
use crate::memory::retrieval::test_support::{
    insert_chunks, insert_score, sample_chunk, test_config,
};

#[test]
fn empty_input_returns_empty() {
    let (_tmp, cfg) = test_config();
    let out = fetch_leaves(&cfg, &[]).unwrap();
    assert!(out.is_empty());
}

#[test]
fn returns_existing_chunks_in_order() {
    let (_tmp, cfg) = test_config();
    let c1 = sample_chunk("slack:#eng", 0, "content-0");
    let c2 = sample_chunk("slack:#eng", 1, "content-1");
    insert_chunks(&cfg, &[c1.clone(), c2.clone()]);
    let out = fetch_leaves(&cfg, &[c1.id.clone(), c2.id.clone()]).unwrap();
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].node_id, c1.id);
    assert_eq!(out[1].node_id, c2.id);
}

#[test]
fn missing_ids_are_skipped() {
    let (_tmp, cfg) = test_config();
    let c1 = sample_chunk("slack:#eng", 0, "content-0");
    insert_chunks(&cfg, std::slice::from_ref(&c1));
    let out = fetch_leaves(
        &cfg,
        &[c1.id.clone(), "ghost:nonexistent".into(), c1.id.clone()],
    )
    .unwrap();
    assert_eq!(out.len(), 2);
    assert!(out.iter().all(|h| h.node_id == c1.id));
}

#[test]
fn over_cap_is_truncated() {
    let (_tmp, cfg) = test_config();
    let mut ids: Vec<String> = Vec::new();
    for i in 0..(MAX_BATCH + 5) as u32 {
        let c = sample_chunk("slack:#eng", i, &format!("content-{i}"));
        insert_chunks(&cfg, std::slice::from_ref(&c));
        ids.push(c.id);
    }
    let out = fetch_leaves(&cfg, &ids).unwrap();
    assert_eq!(out.len(), MAX_BATCH);
}

#[test]
fn leaf_hit_carries_source_ref_and_scope() {
    let (_tmp, cfg) = test_config();
    let c = sample_chunk("slack:#eng", 0, "content-0");
    insert_chunks(&cfg, std::slice::from_ref(&c));
    let out = fetch_leaves(&cfg, std::slice::from_ref(&c.id)).unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].source_ref.as_deref(), Some("slack://slack:#eng/0"));
    assert_eq!(out[0].tree_scope, "slack:#eng");
}

#[test]
fn fetch_leaves_preserves_input_order_and_propagates_scores() {
    let (_tmp, cfg) = test_config();
    let c1 = sample_chunk("slack:#eng", 0, "content-0");
    let c2 = sample_chunk("slack:#eng", 1, "content-1");
    let c3 = sample_chunk("slack:#eng", 2, "content-2");
    insert_chunks(&cfg, &[c1.clone(), c2.clone(), c3.clone()]);

    insert_score(&cfg, &c1.id, 0.1);
    insert_score(&cfg, &c2.id, 0.2);
    // c3 intentionally has NO score row → 0.0 fallback.

    // Request order: c2, ghost, c3, c1 — none in natural id order.
    let out = fetch_leaves(
        &cfg,
        &[
            c2.id.clone(),
            "ghost:no-such".into(),
            c3.id.clone(),
            c1.id.clone(),
        ],
    )
    .unwrap();
    assert_eq!(out.len(), 3, "ghost dropped, 3 real chunks returned");
    assert_eq!(out[0].node_id, c2.id);
    assert_eq!(out[1].node_id, c3.id);
    assert_eq!(out[2].node_id, c1.id);
    assert!((out[0].score - 0.2).abs() < 1e-6, "c2 score");
    assert!(
        out[1].score.abs() < 1e-6,
        "c3 has no score row → 0.0 fallback"
    );
    assert!((out[2].score - 0.1).abs() < 1e-6, "c1 score");
}

#[test]
fn fetch_leaves_hydrates_full_staged_body_instead_of_sql_preview() {
    let (_tmp, cfg) = test_config();
    let full_body = "full-body ".repeat(120);
    let chunk = sample_chunk("slack:#eng", 0, &full_body);
    let staged = crate::memory::store::content::stage_chunks(
        &crate::memory::chunks::content_root(&cfg),
        std::slice::from_ref(&chunk),
    )
    .unwrap();
    crate::memory::chunks::with_connection(&cfg, |conn| {
        let tx = conn.unchecked_transaction()?;
        crate::memory::chunks::upsert_staged_chunks_tx(&tx, &staged)?;
        tx.commit()?;
        Ok(())
    })
    .unwrap();

    let out = fetch_leaves(&cfg, &[chunk.id]).unwrap();
    assert_eq!(out[0].content, full_body);
    assert!(out[0].content.len() > 500);
}
