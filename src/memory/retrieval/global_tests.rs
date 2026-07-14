//! `query_global` and `query_topic` tests.

use super::*;
use crate::memory::config::MemoryConfig;
use crate::memory::retrieval::test_support::{
    fixed_ts, index_entity_occurrence, insert_chunks, insert_score, insert_summary,
    insert_tree_row, sample_chunk, sample_chunk_at, source_tree, summary_node, test_config,
};
use crate::memory::score::embed::InertEmbedder;
use crate::memory::score::extract::EntityKind;
use chrono::{DateTime, TimeZone, Utc};

fn inert() -> InertEmbedder {
    InertEmbedder::new()
}

#[test]
fn millisecond_conversion_saturates_out_of_range_values() {
    assert_eq!(ms_to_utc(i64::MIN), DateTime::<Utc>::MIN_UTC);
    assert_eq!(ms_to_utc(i64::MAX), DateTime::<Utc>::MAX_UTC);
    assert_eq!(
        ms_to_utc(1_700_000_000_000),
        Utc.timestamp_millis_opt(1_700_000_000_000).unwrap()
    );
}

/// Seed a source tree at `scope` with one L1 summary `summary_id` at `ts`.
fn seed_tree(cfg: &MemoryConfig, scope: &str, summary_id: &str, ts: DateTime<Utc>) {
    let tree_id = format!("tree:{scope}");
    let tree = source_tree(&tree_id, scope, Some(summary_id), 1);
    insert_tree_row(cfg, &tree);
    let node = summary_node(summary_id, &tree_id, 1, None, &["leaf-a"], "summary", ts);
    insert_summary(cfg, &node);
}

// ── query_global ────────────────────────────────────────────────────────────

#[tokio::test]
async fn query_global_empty_store_returns_empty() {
    let (_tmp, cfg) = test_config();
    let resp = query_global(&cfg, 0, i64::MAX / 2, None, None, &inert(), 10)
        .await
        .unwrap();
    assert!(resp.hits.is_empty());
}

#[tokio::test]
async fn query_global_until_before_since_errors() {
    let (_tmp, cfg) = test_config();
    assert!(query_global(&cfg, 200, 100, None, None, &inert(), 10)
        .await
        .is_err());
}

#[tokio::test]
async fn query_global_gathers_across_sources_in_window() {
    let (_tmp, cfg) = test_config();
    let ts = fixed_ts();
    seed_tree(&cfg, "slack:#eng", "s:eng", ts);
    seed_tree(&cfg, "gmail:alice", "s:gmail", ts);

    let ms = ts.timestamp_millis();
    let resp = query_global(&cfg, ms - 1, ms + 1, None, None, &inert(), 10)
        .await
        .unwrap();
    assert_eq!(
        resp.hits.len(),
        2,
        "both source summaries fall in the window"
    );
}

#[tokio::test]
async fn query_global_drops_out_of_window_summaries() {
    let (_tmp, cfg) = test_config();
    let old = Utc.timestamp_millis_opt(1_000_000_000_000).unwrap();
    let new = fixed_ts();
    seed_tree(&cfg, "slack:#old", "s:old", old);
    seed_tree(&cfg, "slack:#new", "s:new", new);

    let ms = new.timestamp_millis();
    let resp = query_global(&cfg, ms - 1, ms + 1, None, None, &inert(), 10)
        .await
        .unwrap();
    assert_eq!(resp.hits.len(), 1);
    assert_eq!(resp.hits[0].tree_scope, "slack:#new");
}

// ── query_topic ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn query_topic_empty_entity_returns_empty() {
    let (_tmp, cfg) = test_config();
    let resp = query_topic(&cfg, "   ", None, None, None, &inert(), 10)
        .await
        .unwrap();
    assert!(resp.hits.is_empty());
}

#[tokio::test]
async fn query_topic_unknown_entity_returns_empty() {
    let (_tmp, cfg) = test_config();
    let resp = query_topic(&cfg, "topic:nope", None, None, None, &inert(), 10)
        .await
        .unwrap();
    assert!(resp.hits.is_empty());
}

#[tokio::test]
async fn query_topic_resolves_indexed_summary_with_scope() {
    let (_tmp, cfg) = test_config();
    let ts = fixed_ts();
    seed_tree(&cfg, "slack:#eng", "s:eng", ts);
    index_entity_occurrence(
        &cfg,
        "topic:phoenix",
        EntityKind::Topic,
        "phoenix",
        "s:eng",
        "summary",
        ts.timestamp_millis(),
        Some("tree:slack:#eng"),
    );

    let resp = query_topic(&cfg, "topic:phoenix", None, None, None, &inert(), 10)
        .await
        .unwrap();
    assert_eq!(resp.hits.len(), 1);
    assert_eq!(resp.hits[0].node_id, "s:eng");
    assert_eq!(resp.hits[0].tree_scope, "slack:#eng");
}

#[tokio::test]
async fn query_topic_resolves_indexed_leaf() {
    let (_tmp, cfg) = test_config();
    let ts = fixed_ts();
    let chunk = sample_chunk("slack:#eng", 0, "phoenix launch notes");
    insert_chunks(&cfg, std::slice::from_ref(&chunk));
    index_entity_occurrence(
        &cfg,
        "topic:phoenix",
        EntityKind::Topic,
        "phoenix",
        &chunk.id,
        "leaf",
        ts.timestamp_millis(),
        None,
    );

    let resp = query_topic(&cfg, "topic:phoenix", None, None, None, &inert(), 10)
        .await
        .unwrap();
    assert_eq!(resp.hits.len(), 1);
    assert_eq!(resp.hits[0].node_id, chunk.id);
    assert_eq!(resp.hits[0].node_kind, super::super::types::NodeKind::Leaf);
    assert_eq!(resp.hits[0].tree_scope, "slack:#eng");
}

#[tokio::test]
async fn query_topic_window_filters_old_nodes() {
    let (_tmp, cfg) = test_config();
    let old = Utc.timestamp_millis_opt(1_000_000_000_000).unwrap();
    let new = fixed_ts();
    seed_tree(&cfg, "slack:#old", "s:old", old);
    seed_tree(&cfg, "slack:#new", "s:new", new);
    index_entity_occurrence(
        &cfg,
        "topic:p",
        EntityKind::Topic,
        "p",
        "s:old",
        "summary",
        old.timestamp_millis(),
        None,
    );
    index_entity_occurrence(
        &cfg,
        "topic:p",
        EntityKind::Topic,
        "p",
        "s:new",
        "summary",
        new.timestamp_millis(),
        None,
    );

    let ms = new.timestamp_millis();
    let resp = query_topic(
        &cfg,
        "topic:p",
        Some(ms - 1),
        Some(ms + 1),
        None,
        &inert(),
        10,
    )
    .await
    .unwrap();
    assert_eq!(resp.hits.len(), 1);
    assert_eq!(resp.hits[0].node_id, "s:new");
}

#[tokio::test]
async fn query_topic_applies_historical_window_before_lookup_cap() {
    use crate::memory::chunks::with_connection;

    let (_tmp, cfg) = test_config();
    let old = Utc.timestamp_millis_opt(1_000_000_000_000).unwrap();
    let chunk = sample_chunk_at("slack:#history", 0, "historic phoenix", old);
    insert_chunks(&cfg, std::slice::from_ref(&chunk));
    index_entity_occurrence(
        &cfg,
        "topic:phoenix",
        EntityKind::Topic,
        "phoenix",
        &chunk.id,
        "leaf",
        old.timestamp_millis(),
        None,
    );
    // More than the configured topic cap recent stale index rows used to hide the old
    // real row because LIMIT ran before the historical-window filter.
    with_connection(&cfg, |conn| {
        let tx = conn.unchecked_transaction()?;
        for index in 0..=cfg.retrieval.limits.topic_lookup_limit {
            tx.execute(
                "INSERT INTO mem_tree_entity_index
                 (entity_id,node_id,node_kind,entity_kind,surface,score,timestamp_ms,tree_id,is_user)
                 VALUES ('topic:phoenix',?1,'leaf','topic','phoenix',1.0,?2,NULL,0)",
                rusqlite::params![
                    format!("stale-{index}"),
                    fixed_ts().timestamp_millis() + index as i64
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    })
    .unwrap();

    let response = query_topic(
        &cfg,
        "topic:phoenix",
        Some(old.timestamp_millis() - 1),
        Some(old.timestamp_millis() + 1),
        None,
        &inert(),
        10,
    )
    .await
    .unwrap();

    assert_eq!(response.hits.len(), 1);
    assert_eq!(response.hits[0].node_id, chunk.id);
}

#[tokio::test]
async fn query_topic_excludes_chunks_with_dropped_score_rows() {
    use crate::memory::chunks::with_connection;

    let (_tmp, cfg) = test_config();
    let chunk = sample_chunk("slack:#eng", 0, "phoenix");
    insert_chunks(&cfg, std::slice::from_ref(&chunk));
    insert_score(&cfg, &chunk.id, 0.1);
    with_connection(&cfg, |conn| {
        conn.execute(
            "UPDATE mem_tree_score SET dropped=1 WHERE chunk_id=?1",
            rusqlite::params![chunk.id],
        )?;
        Ok(())
    })
    .unwrap();
    index_entity_occurrence(
        &cfg,
        "topic:phoenix",
        EntityKind::Topic,
        "phoenix",
        &chunk.id,
        "leaf",
        fixed_ts().timestamp_millis(),
        None,
    );

    let response = query_topic(&cfg, "topic:phoenix", None, None, None, &inert(), 10)
        .await
        .unwrap();

    assert!(response.hits.is_empty());
}
