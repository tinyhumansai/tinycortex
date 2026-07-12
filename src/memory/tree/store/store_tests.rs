//! Round-trip tests for tree / summary / buffer persistence, embedding sidecar
//! scoping, stale-buffer queries, and the list/archive registry helpers.
//!
//! Adapted from OpenHuman's `memory_store/trees/store_tests.rs`:
//! `Config` → [`MemoryConfig`], and `insert_summary_tx` drops the staged-content
//! argument (TinyCortex stores full summary bodies inline, no on-disk staging).

use super::*;
use chrono::{TimeZone, Utc};
use tempfile::TempDir;

use crate::memory::chunks::with_connection;
use crate::memory::config::MemoryConfig;
use crate::memory::store::content::StagedSummary;

fn test_config() -> (TempDir, MemoryConfig) {
    let tmp = TempDir::new().unwrap();
    let cfg = MemoryConfig::new(tmp.path());
    (tmp, cfg)
}

fn sample_tree(id: &str, scope: &str) -> Tree {
    Tree {
        id: id.to_string(),
        kind: TreeKind::Source,
        scope: scope.to_string(),
        root_id: None,
        max_level: 0,
        status: TreeStatus::Active,
        created_at: Utc.timestamp_millis_opt(1_700_000_000_000).unwrap(),
        last_sealed_at: None,
    }
}

fn sample_summary(id: &str, tree_id: &str, level: u32) -> SummaryNode {
    let ts = Utc.timestamp_millis_opt(1_700_000_000_000).unwrap();
    SummaryNode {
        id: id.to_string(),
        tree_id: tree_id.to_string(),
        tree_kind: TreeKind::Source,
        level,
        parent_id: None,
        child_ids: vec!["leaf-a".into(), "leaf-b".into()],
        content: "seal content".into(),
        token_count: 100,
        entities: vec!["entity:alice".into()],
        topics: vec!["#launch".into()],
        time_range_start: ts,
        time_range_end: ts,
        score: 0.75,
        sealed_at: ts,
        deleted: false,
        embedding: None,
        doc_id: None,
        version_ms: None,
    }
}

#[test]
fn tree_round_trip() {
    let (_tmp, cfg) = test_config();
    let t = sample_tree("tree-1", "slack:#eng");
    insert_tree(&cfg, &t).unwrap();
    let got = get_tree(&cfg, "tree-1").unwrap().unwrap();
    assert_eq!(got, t);
    let by_scope = get_tree_by_scope(&cfg, TreeKind::Source, "slack:#eng")
        .unwrap()
        .unwrap();
    assert_eq!(by_scope.id, "tree-1");
}

#[test]
fn duplicate_scope_fails() {
    let (_tmp, cfg) = test_config();
    insert_tree(&cfg, &sample_tree("t1", "slack:#eng")).unwrap();
    assert!(insert_tree(&cfg, &sample_tree("t2", "slack:#eng")).is_err());
}

#[test]
fn summary_insert_and_fetch() {
    let (_tmp, cfg) = test_config();
    insert_tree(&cfg, &sample_tree("tree-1", "slack:#eng")).unwrap();
    let node = sample_summary("sum-1", "tree-1", 1);
    with_connection(&cfg, |conn| {
        let tx = conn.unchecked_transaction()?;
        insert_summary_tx(&tx, &node, "test")?;
        tx.commit()?;
        Ok(())
    })
    .unwrap();
    let got = get_summary(&cfg, "sum-1").unwrap().unwrap();
    assert_eq!(got, node);
    assert_eq!(list_summaries_at_level(&cfg, "tree-1", 1).unwrap().len(), 1);
    assert_eq!(count_summaries(&cfg, "tree-1").unwrap(), 1);
}

#[test]
fn staged_summary_persists_preview_and_content_pointer_atomically() {
    let (_tmp, cfg) = test_config();
    insert_tree(&cfg, &sample_tree("tree-1", "slack:#eng")).unwrap();
    let mut node = sample_summary("sum-staged", "tree-1", 1);
    node.content = "x".repeat(700);
    let staged = StagedSummary {
        summary_id: node.id.clone(),
        content_path: "summaries/tree-1/sum-staged.md".into(),
        content_sha256: "abc123".into(),
    };
    with_connection(&cfg, |conn| {
        let tx = conn.unchecked_transaction()?;
        insert_staged_summary_tx(&tx, &node, Some(&staged), "test")?;
        tx.commit()?;
        let row: (String, String, String) = conn.query_row(
            "SELECT content, content_path, content_sha256 FROM mem_tree_summaries WHERE id=?1",
            [&node.id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        assert_eq!(row.0.len(), 500);
        assert_eq!(row.1, staged.content_path);
        assert_eq!(row.2, staged.content_sha256);
        Ok(())
    })
    .unwrap();
}

#[test]
fn cascade_delete_returns_staged_paths_and_removes_tree_rows() {
    let (_tmp, cfg) = test_config();
    insert_tree(&cfg, &sample_tree("tree-delete", "slack:#delete")).unwrap();
    let node = sample_summary("sum-delete", "tree-delete", 1);
    let staged = StagedSummary {
        summary_id: node.id.clone(),
        content_path: "summaries/tree-delete/sum-delete.md".into(),
        content_sha256: "abc123".into(),
    };
    let deleted = with_connection(&cfg, |conn| {
        let tx = conn.unchecked_transaction()?;
        insert_staged_summary_tx(&tx, &node, Some(&staged), "test")?;
        let deleted = delete_tree_cascade_tx(&tx, "tree-delete")?;
        tx.commit()?;
        Ok(deleted)
    })
    .unwrap();
    assert_eq!(deleted.removed_summaries, 1);
    assert_eq!(deleted.content_paths, vec![staged.content_path]);
    assert!(get_tree(&cfg, "tree-delete").unwrap().is_none());
    assert!(get_summary(&cfg, "sum-delete").unwrap().is_none());
}

#[test]
fn list_summaries_in_window_keeps_only_fully_contained() {
    let (_tmp, cfg) = test_config();
    insert_tree(&cfg, &sample_tree("tree-1", "slack:#eng")).unwrap();
    let mk = |id: &str, start_ms: i64, end_ms: i64| {
        let mut n = sample_summary(id, "tree-1", 1);
        n.time_range_start = Utc.timestamp_millis_opt(start_ms).unwrap();
        n.time_range_end = Utc.timestamp_millis_opt(end_ms).unwrap();
        n
    };
    let inside = mk("inside", 1100, 1900);
    let straddle_start = mk("straddle", 900, 1500);
    let straddle_end = mk("overrun", 1500, 2100);
    let outside = mk("outside", 3000, 3500);
    with_connection(&cfg, |conn| {
        let tx = conn.unchecked_transaction()?;
        for n in [&inside, &straddle_start, &straddle_end, &outside] {
            insert_summary_tx(&tx, n, "test")?;
        }
        tx.commit()?;
        Ok(())
    })
    .unwrap();
    let eligible = list_summaries_in_window(&cfg, "tree-1", 1000, 2000).unwrap();
    let ids: Vec<&str> = eligible.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, vec!["inside"]);
}

#[test]
fn list_summaries_in_window_includes_exact_boundaries() {
    let (_tmp, cfg) = test_config();
    insert_tree(&cfg, &sample_tree("tree-1", "slack:#eng")).unwrap();
    let mk = |id: &str, start_ms: i64, end_ms: i64| {
        let mut n = sample_summary(id, "tree-1", 1);
        n.time_range_start = Utc.timestamp_millis_opt(start_ms).unwrap();
        n.time_range_end = Utc.timestamp_millis_opt(end_ms).unwrap();
        n
    };
    let on_start = mk("start-edge", 1000, 1500);
    let on_end = mk("end-edge", 1500, 2000);
    let both = mk("both-edges", 1000, 2000);
    with_connection(&cfg, |conn| {
        let tx = conn.unchecked_transaction()?;
        for n in [&on_start, &on_end, &both] {
            insert_summary_tx(&tx, n, "test")?;
        }
        tx.commit()?;
        Ok(())
    })
    .unwrap();
    let eligible = list_summaries_in_window(&cfg, "tree-1", 1000, 2000).unwrap();
    let mut ids: Vec<&str> = eligible.iter().map(|s| s.id.as_str()).collect();
    ids.sort_unstable();
    assert_eq!(ids, vec!["both-edges", "end-edge", "start-edge"]);
}

#[test]
fn list_summaries_in_window_excludes_deleted() {
    let (_tmp, cfg) = test_config();
    insert_tree(&cfg, &sample_tree("tree-1", "slack:#eng")).unwrap();
    let mut node = sample_summary("sum-1", "tree-1", 1);
    node.time_range_start = Utc.timestamp_millis_opt(1100).unwrap();
    node.time_range_end = Utc.timestamp_millis_opt(1900).unwrap();
    node.deleted = true;
    with_connection(&cfg, |conn| {
        let tx = conn.unchecked_transaction()?;
        insert_summary_tx(&tx, &node, "test")?;
        tx.commit()?;
        Ok(())
    })
    .unwrap();
    assert!(list_summaries_in_window(&cfg, "tree-1", 1000, 2000)
        .unwrap()
        .is_empty());
}

#[test]
fn summary_insert_is_idempotent_on_id() {
    let (_tmp, cfg) = test_config();
    insert_tree(&cfg, &sample_tree("tree-1", "slack:#eng")).unwrap();
    let node = sample_summary("sum-1", "tree-1", 1);
    with_connection(&cfg, |conn| {
        let tx = conn.unchecked_transaction()?;
        insert_summary_tx(&tx, &node, "test")?;
        insert_summary_tx(&tx, &node, "test")?;
        tx.commit()?;
        Ok(())
    })
    .unwrap();
    assert_eq!(count_summaries(&cfg, "tree-1").unwrap(), 1);
}

#[test]
fn summary_embeddings_are_scoped_by_model_signature() {
    let (_tmp, cfg) = test_config();
    insert_tree(&cfg, &sample_tree("tree-1", "slack:#eng")).unwrap();
    let node = sample_summary("sum-embed", "tree-1", 1);
    with_connection(&cfg, |conn| {
        let tx = conn.unchecked_transaction()?;
        insert_summary_tx(&tx, &node, "test")?;
        tx.commit()?;
        Ok(())
    })
    .unwrap();

    set_summary_embedding_for_signature(
        &cfg,
        "sum-embed",
        "openai/text-embedding-3-small@1536",
        &[0.1, 0.2],
    )
    .unwrap();
    set_summary_embedding_for_signature(&cfg, "sum-embed", "local/bge-small@384", &[0.3, 0.4, 0.5])
        .unwrap();

    assert_eq!(
        get_summary_embedding_for_signature(
            &cfg,
            "sum-embed",
            "openai/text-embedding-3-small@1536"
        )
        .unwrap(),
        Some(vec![0.1, 0.2])
    );
    assert_eq!(
        get_summary_embedding_for_signature(&cfg, "sum-embed", "local/bge-small@384").unwrap(),
        Some(vec![0.3, 0.4, 0.5])
    );
    assert!(
        get_summary_embedding_for_signature(&cfg, "sum-embed", "missing/model@1")
            .unwrap()
            .is_none()
    );

    // Nothing written under the active signature yet → absent.
    assert!(get_summary_embedding(&cfg, "sum-embed").unwrap().is_none());

    // The public setter targets the active signature and round-trips.
    set_summary_embedding(&cfg, "sum-embed", &[0.7, 0.8]).unwrap();
    assert_eq!(
        get_summary_embedding(&cfg, "sum-embed").unwrap(),
        Some(vec![0.7, 0.8])
    );
    // Earlier per-signature rows stay independently scoped.
    assert_eq!(
        get_summary_embedding_for_signature(&cfg, "sum-embed", "local/bge-small@384").unwrap(),
        Some(vec![0.3, 0.4, 0.5])
    );
}

#[test]
fn buffer_upsert_and_consume_snapshot() {
    let (_tmp, cfg) = test_config();
    insert_tree(&cfg, &sample_tree("tree-1", "slack:#eng")).unwrap();
    let ts = Utc.timestamp_millis_opt(1_700_000_000_000).unwrap();
    let buf = Buffer {
        tree_id: "tree-1".into(),
        level: 0,
        item_ids: vec!["leaf-a".into(), "leaf-b".into()],
        token_sum: 500,
        oldest_at: Some(ts),
    };
    with_connection(&cfg, |conn| {
        let tx = conn.unchecked_transaction()?;
        upsert_buffer_tx(&tx, &buf)?;
        tx.commit()?;
        Ok(())
    })
    .unwrap();
    assert_eq!(get_buffer(&cfg, "tree-1", 0).unwrap(), buf);

    with_connection(&cfg, |conn| {
        let tx = conn.unchecked_transaction()?;
        consume_snapshot_tx(&tx, &buf)?;
        tx.commit()?;
        Ok(())
    })
    .unwrap();
    let cleared = get_buffer(&cfg, "tree-1", 0).unwrap();
    assert!(cleared.is_empty());
    assert_eq!(cleared.token_sum, 0);
    assert!(cleared.oldest_at.is_none());
}

#[test]
fn get_buffer_returns_empty_when_missing() {
    let (_tmp, cfg) = test_config();
    insert_tree(&cfg, &sample_tree("tree-1", "slack:#eng")).unwrap();
    let got = get_buffer(&cfg, "tree-1", 0).unwrap();
    assert!(got.is_empty());
    assert_eq!(got.tree_id, "tree-1");
}

#[test]
fn update_tree_after_seal_persists() {
    let (_tmp, cfg) = test_config();
    insert_tree(&cfg, &sample_tree("tree-1", "slack:#eng")).unwrap();
    let sealed_at = Utc.timestamp_millis_opt(1_700_000_123_000).unwrap();
    with_connection(&cfg, |conn| {
        let tx = conn.unchecked_transaction()?;
        update_tree_after_seal_tx(&tx, "tree-1", "sum-1", 1, sealed_at)?;
        tx.commit()?;
        Ok(())
    })
    .unwrap();
    let got = get_tree(&cfg, "tree-1").unwrap().unwrap();
    assert_eq!(got.root_id.as_deref(), Some("sum-1"));
    assert_eq!(got.max_level, 1);
    assert_eq!(got.last_sealed_at, Some(sealed_at));
}

#[test]
fn list_stale_buffers_orders_by_age() {
    let (_tmp, cfg) = test_config();
    insert_tree(&cfg, &sample_tree("tree-1", "slack:#eng")).unwrap();
    insert_tree(&cfg, &sample_tree("tree-2", "slack:#ops")).unwrap();
    let t0 = Utc.timestamp_millis_opt(1_700_000_000_000).unwrap();
    let t1 = Utc.timestamp_millis_opt(1_700_000_010_000).unwrap();
    let t_l1 = Utc.timestamp_millis_opt(1_700_000_005_000).unwrap();
    let t2 = Utc.timestamp_millis_opt(1_700_000_020_000).unwrap();
    with_connection(&cfg, |conn| {
        let tx = conn.unchecked_transaction()?;
        upsert_buffer_tx(
            &tx,
            &Buffer {
                tree_id: "tree-1".into(),
                level: 0,
                item_ids: vec!["a".into()],
                token_sum: 10,
                oldest_at: Some(t0),
            },
        )?;
        upsert_buffer_tx(
            &tx,
            &Buffer {
                tree_id: "tree-1".into(),
                level: 1,
                item_ids: vec!["upper".into()],
                token_sum: 5,
                oldest_at: Some(t_l1),
            },
        )?;
        upsert_buffer_tx(
            &tx,
            &Buffer {
                tree_id: "tree-2".into(),
                level: 0,
                item_ids: vec!["b".into()],
                token_sum: 20,
                oldest_at: Some(t1),
            },
        )?;
        tx.commit()?;
        Ok(())
    })
    .unwrap();
    let stale = list_stale_buffers(&cfg, t2).unwrap();
    assert_eq!(stale.len(), 2, "L1 stale buffer must be filtered out");
    assert!(stale.iter().all(|b| b.level == 0));
    assert_eq!(stale[0].oldest_at, Some(t0));
    assert_eq!(stale[1].oldest_at, Some(t1));
    let only_oldest = list_stale_buffers(&cfg, t0).unwrap();
    assert_eq!(only_oldest.len(), 1);
    assert_eq!(only_oldest[0].tree_id, "tree-1");
}

#[test]
fn get_trees_batch_returns_present_ids_and_skips_missing() {
    let (_tmp, cfg) = test_config();
    assert!(get_trees_batch(&cfg, &[]).unwrap().is_empty());
    let a = sample_tree("tree-a", "slack:#eng");
    let b = sample_tree("tree-b", "slack:#design");
    insert_tree(&cfg, &a).unwrap();
    insert_tree(&cfg, &b).unwrap();
    let ids = vec![
        "tree-a".to_string(),
        "tree-b".to_string(),
        "ghost".to_string(),
    ];
    let map = get_trees_batch(&cfg, &ids).unwrap();
    assert_eq!(map.len(), 2);
    assert_eq!(map.get("tree-a").unwrap(), &a);
    assert_eq!(map.get("tree-b").unwrap(), &b);
    assert!(map.get("ghost").is_none());
}

#[test]
fn get_summaries_batch_returns_present_ids_and_skips_missing() {
    let (_tmp, cfg) = test_config();
    insert_tree(&cfg, &sample_tree("tree-1", "slack:#eng")).unwrap();
    let a = sample_summary("sum-a", "tree-1", 1);
    let b = sample_summary("sum-b", "tree-1", 1);
    with_connection(&cfg, |conn| {
        let tx = conn.unchecked_transaction()?;
        insert_summary_tx(&tx, &a, "test")?;
        insert_summary_tx(&tx, &b, "test")?;
        tx.commit()?;
        Ok(())
    })
    .unwrap();
    assert!(get_summaries_batch(&cfg, &[]).unwrap().is_empty());
    let ids = vec![
        "sum-a".to_string(),
        "sum-b".to_string(),
        "ghost".to_string(),
    ];
    let map = get_summaries_batch(&cfg, &ids).unwrap();
    assert_eq!(map.len(), 2);
    assert_eq!(map.get("sum-a").unwrap(), &a);
    assert_eq!(map.get("sum-b").unwrap(), &b);
}

#[test]
fn summary_batch_embedding_lookup_returns_only_signature_scoped_rows() {
    let (_tmp, cfg) = test_config();
    insert_tree(&cfg, &sample_tree("tree-1", "slack:#eng")).unwrap();
    for sid in ["sum-1", "sum-2", "sum-3"] {
        let node = sample_summary(sid, "tree-1", 1);
        with_connection(&cfg, |conn| {
            let tx = conn.unchecked_transaction()?;
            insert_summary_tx(&tx, &node, "test")?;
            tx.commit()?;
            Ok(())
        })
        .unwrap();
    }
    let sig_a = "openai/text-embedding-3-small@1536";
    let sig_b = "local/bge-small@384";
    set_summary_embedding_for_signature(&cfg, "sum-1", sig_a, &[0.1, 0.2]).unwrap();
    set_summary_embedding_for_signature(&cfg, "sum-2", sig_a, &[0.3, 0.4]).unwrap();
    set_summary_embedding_for_signature(&cfg, "sum-3", sig_b, &[0.5, 0.6, 0.7]).unwrap();

    let ids = vec!["sum-1".into(), "sum-2".into(), "sum-3".into()];
    let map_a = get_summary_embeddings_for_signature_batch(&cfg, &ids, sig_a).unwrap();
    assert_eq!(map_a.len(), 2);
    assert_eq!(map_a.get("sum-1").cloned(), Some(vec![0.1, 0.2]));
    assert!(map_a.get("sum-3").is_none());

    let map_b = get_summary_embeddings_for_signature_batch(&cfg, &ids, sig_b).unwrap();
    assert_eq!(map_b.len(), 1);
    assert_eq!(map_b.get("sum-3").cloned(), Some(vec![0.5, 0.6, 0.7]));

    assert!(get_summary_embeddings_for_signature_batch(&cfg, &[], sig_a)
        .unwrap()
        .is_empty());
    assert!(get_summary_embeddings_batch(&cfg, &ids).unwrap().is_empty());
}

#[test]
fn list_trees_by_kind_and_archive() {
    let (_tmp, cfg) = test_config();
    // Distinct created_at so the `ORDER BY created_at_ms ASC` is deterministic.
    let mut s1 = sample_tree("source-1", "chat:slack:#eng");
    s1.created_at = Utc.timestamp_millis_opt(1_700_000_000_000).unwrap();
    insert_tree(&cfg, &s1).unwrap();
    let mut topic = sample_tree("topic-1", "person:alice");
    topic.kind = TreeKind::Topic;
    insert_tree(&cfg, &topic).unwrap();
    let mut s2 = sample_tree("source-2", "chat:discord:#ops");
    s2.created_at = Utc.timestamp_millis_opt(1_700_000_001_000).unwrap();
    insert_tree(&cfg, &s2).unwrap();

    let source_ids: Vec<String> = list_trees_by_kind(&cfg, TreeKind::Source)
        .unwrap()
        .into_iter()
        .map(|t| t.id)
        .collect();
    assert_eq!(source_ids, vec!["source-1", "source-2"]);
    assert_eq!(
        list_trees_by_kind(&cfg, TreeKind::Topic).unwrap()[0].id,
        "topic-1"
    );

    archive_tree(&cfg, "source-1").unwrap();
    let archived = get_tree(&cfg, "source-1").unwrap().unwrap();
    assert_eq!(archived.status, TreeStatus::Archived);
}
