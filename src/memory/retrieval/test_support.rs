//! Shared test fixtures for the retrieval unit tests.
//!
//! Seeds a small SQLite store under a `tempfile::TempDir` and exposes helpers
//! to insert chunks, scores, summary trees, and entity-index rows directly —
//! bypassing the full ingest/seal pipeline so each retrieval primitive can be
//! exercised against a hand-built fixture with inert (zero-vector) embeddings.

use chrono::{DateTime, TimeZone, Utc};
use tempfile::TempDir;

use crate::memory::chunks::{
    chunk_id, tree_active_signature, upsert_chunks, with_connection, Chunk, Metadata, SourceKind,
    SourceRef,
};
use crate::memory::config::MemoryConfig;
use crate::memory::score::extract::EntityKind;
use crate::memory::score::resolver::CanonicalEntity;
use crate::memory::score::signals::ScoreSignals;
use crate::memory::score::store::{index_entities, upsert_score, ScoreRow};
use crate::memory::tree::store::{
    insert_summary_tx, insert_tree, set_summary_embedding_for_signature,
};
use crate::memory::tree::{SummaryNode, Tree, TreeKind, TreeStatus};

/// A fresh config rooted at a throwaway tempdir. The returned [`TempDir`] must
/// be kept alive for the duration of the test (dropping it deletes the store).
pub fn test_config() -> (TempDir, MemoryConfig) {
    let tmp = TempDir::new().unwrap();
    let cfg = MemoryConfig::new(tmp.path());
    (tmp, cfg)
}

/// A fixed epoch-millis timestamp for deterministic fixtures.
pub fn fixed_ts() -> DateTime<Utc> {
    Utc.timestamp_millis_opt(1_700_000_000_000).unwrap()
}

/// Build a chat chunk with a deterministic id derived from its content.
pub fn sample_chunk(source: &str, seq: u32, content: &str) -> Chunk {
    sample_chunk_at(source, seq, content, fixed_ts())
}

/// Build a chat chunk at an explicit timestamp.
pub fn sample_chunk_at(source: &str, seq: u32, content: &str, ts: DateTime<Utc>) -> Chunk {
    Chunk {
        id: chunk_id(SourceKind::Chat, source, seq, content),
        content: content.to_string(),
        metadata: Metadata {
            source_kind: SourceKind::Chat,
            source_id: source.into(),
            owner: "alice".into(),
            timestamp: ts,
            time_range: (ts, ts),
            tags: vec![],
            source_ref: Some(SourceRef::new(format!("slack://{source}/{seq}"))),
            path_scope: None,
        },
        token_count: 20,
        seq_in_source: seq,
        created_at: ts,
        partial_message: false,
    }
}

/// Persist chunks to the store.
pub fn insert_chunks(cfg: &MemoryConfig, chunks: &[Chunk]) {
    upsert_chunks(cfg, chunks).unwrap();
}

/// Upsert a score row for a chunk with the given total.
pub fn insert_score(cfg: &MemoryConfig, chunk_id: &str, total: f32) {
    upsert_score(
        cfg,
        &ScoreRow {
            chunk_id: chunk_id.to_string(),
            total,
            signals: ScoreSignals::default(),
            dropped: false,
            reason: None,
            computed_at_ms: 0,
            llm_importance_reason: None,
        },
    )
    .unwrap();
}

/// Insert a tree row.
pub fn insert_tree_row(cfg: &MemoryConfig, tree: &Tree) {
    insert_tree(cfg, tree).unwrap();
}

/// Insert a summary node row (idempotent on primary key). Any `embedding` on
/// the node is written to the active-signature sidecar.
pub fn insert_summary(cfg: &MemoryConfig, node: &SummaryNode) {
    let sig = tree_active_signature(cfg);
    with_connection(cfg, |conn| {
        let tx = conn.unchecked_transaction()?;
        insert_summary_tx(&tx, node, &sig)?;
        tx.commit()?;
        Ok(())
    })
    .unwrap();
}

/// Write a summary's embedding to the active-signature sidecar table.
pub fn set_summary_embedding(cfg: &MemoryConfig, summary_id: &str, vec: &[f32]) {
    let sig = tree_active_signature(cfg);
    set_summary_embedding_for_signature(cfg, summary_id, &sig, vec).unwrap();
}

/// Index one entity occurrence against a node.
#[allow(clippy::too_many_arguments)]
pub fn index_entity_occurrence(
    cfg: &MemoryConfig,
    canonical_id: &str,
    kind: EntityKind,
    surface: &str,
    node_id: &str,
    node_kind: &str,
    timestamp_ms: i64,
    tree_id: Option<&str>,
) {
    let entity = CanonicalEntity {
        canonical_id: canonical_id.to_string(),
        kind,
        surface: surface.to_string(),
        span_start: 0,
        span_end: 0,
        score: 1.0,
    };
    index_entities(cfg, &[entity], node_id, node_kind, timestamp_ms, tree_id).unwrap();
}

/// A minimal active source tree rooted at `scope`.
pub fn source_tree(id: &str, scope: &str, root_id: Option<&str>, max_level: u32) -> Tree {
    let ts = fixed_ts();
    Tree {
        id: id.into(),
        kind: TreeKind::Source,
        scope: scope.into(),
        root_id: root_id.map(|s| s.to_string()),
        max_level,
        status: TreeStatus::Active,
        created_at: ts,
        last_sealed_at: Some(ts),
        ask: None,
    }
}

/// A summary node builder. `embedding` is `None` by default; set it before
/// inserting to populate the sidecar.
#[allow(clippy::too_many_arguments)]
pub fn summary_node(
    id: &str,
    tree_id: &str,
    level: u32,
    parent_id: Option<&str>,
    child_ids: &[&str],
    content: &str,
    ts: DateTime<Utc>,
) -> SummaryNode {
    SummaryNode {
        id: id.into(),
        tree_id: tree_id.into(),
        tree_kind: TreeKind::Source,
        level,
        parent_id: parent_id.map(|s| s.to_string()),
        child_ids: child_ids.iter().map(|s| s.to_string()).collect(),
        content: content.into(),
        token_count: 10,
        entities: vec![],
        topics: vec![],
        time_range_start: ts,
        time_range_end: ts,
        score: 0.5,
        sealed_at: ts,
        deleted: false,
        embedding: None,
        doc_id: None,
        version_ms: None,
    }
}
