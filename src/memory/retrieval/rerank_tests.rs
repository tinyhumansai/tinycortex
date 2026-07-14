use anyhow::Result;
use async_trait::async_trait;
use chrono::{TimeZone, Utc};

use super::*;
use crate::memory::retrieval::types::NodeKind;
use crate::memory::tree::TreeKind;

struct FixedEmbedder;

#[async_trait]
impl Embedder for FixedEmbedder {
    fn name(&self) -> &'static str {
        "fixed"
    }

    async fn embed(&self, _text: &str) -> Result<Vec<f32>> {
        Ok(vec![1.0, 0.0])
    }
}

fn hit(id: &str, timestamp_ms: i64) -> RetrievalHit {
    let timestamp = Utc.timestamp_millis_opt(timestamp_ms).unwrap();
    RetrievalHit {
        node_id: id.into(),
        node_kind: NodeKind::Summary,
        tree_id: "tree".into(),
        tree_kind: TreeKind::Source,
        tree_scope: "scope".into(),
        level: 1,
        content: id.into(),
        entities: vec![],
        topics: vec![],
        time_range_start: timestamp,
        time_range_end: timestamp,
        score: 0.0,
        child_ids: vec![],
        source_ref: None,
    }
}

#[tokio::test]
async fn rerank_orders_embedded_hits_by_similarity_descending() {
    let hits = vec![hit("opposite", 3), hit("same", 1), hit("orthogonal", 2)];
    let embeddings = vec![
        Some(vec![-1.0, 0.0]),
        Some(vec![1.0, 0.0]),
        Some(vec![0.0, 1.0]),
    ];
    let ranked = rerank_by_semantic_similarity(&FixedEmbedder, "q", hits, embeddings).await;
    let ids: Vec<_> = ranked.iter().map(|hit| hit.node_id.as_str()).collect();
    assert_eq!(ids, vec!["same", "orthogonal", "opposite"]);
}

#[tokio::test]
async fn rerank_preserves_incoming_order_for_unembedded_tail() {
    let hits = vec![
        hit("old-unembedded", 1),
        hit("ranked", 2),
        hit("new-unembedded", 3),
    ];
    let embeddings = vec![None, Some(vec![1.0, 0.0]), None];
    let ranked = rerank_by_semantic_similarity(&FixedEmbedder, "q", hits, embeddings).await;
    let ids: Vec<_> = ranked.iter().map(|hit| hit.node_id.as_str()).collect();
    assert_eq!(ids, vec!["ranked", "old-unembedded", "new-unembedded"]);
}

#[tokio::test]
async fn rerank_treats_dimension_mismatch_as_unembedded() {
    let hits = vec![hit("mismatch", 3), hit("ranked", 1)];
    let embeddings = vec![Some(vec![1.0]), Some(vec![1.0, 0.0])];
    let ranked = rerank_by_semantic_similarity(&FixedEmbedder, "q", hits, embeddings).await;
    assert_eq!(ranked[0].node_id, "ranked");
    assert_eq!(ranked[1].node_id, "mismatch");
}
