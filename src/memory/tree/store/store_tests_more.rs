use super::*;

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
    assert!(!map_a.contains_key("sum-3"));

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
