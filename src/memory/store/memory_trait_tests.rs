use super::*;

#[tokio::test]
async fn headline_memory_contract_supports_upsert_recall_and_taint() {
    let store = InMemoryMemoryStore::new();
    store
        .store_with_taint(
            "agent",
            "preference",
            "Prefers dark mode",
            MemoryCategory::Core,
            Some("s1"),
            MemoryTaint::ExternalSync,
        )
        .await
        .unwrap();
    store
        .store_with_taint(
            "agent",
            "preference",
            "Prefers dark themes",
            MemoryCategory::Core,
            Some("s1"),
            MemoryTaint::ExternalSync,
        )
        .await
        .unwrap();

    assert_eq!(store.count().await.unwrap(), 1);
    let hits = store
        .recall(
            "dark themes",
            10,
            RecallOpts {
                namespace: Some("agent"),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].key, "preference");
    assert_eq!(hits[0].taint, MemoryTaint::ExternalSync);
    assert_eq!(hits[0].score, Some(1.0));
}

#[tokio::test]
async fn memory_contract_filters_lists_summarises_and_forgets() {
    let store = InMemoryMemoryStore::new();
    store
        .store("a", "one", "alpha", MemoryCategory::Daily, None)
        .await
        .unwrap();
    store
        .store("b", "two", "beta", MemoryCategory::Core, Some("s2"))
        .await
        .unwrap();

    assert_eq!(
        store.list(Some("b"), None, Some("s2")).await.unwrap().len(),
        1
    );
    assert!(store.get("b", "two").await.unwrap().is_some());
    let summaries = store.namespace_summaries().await.unwrap();
    assert_eq!(summaries.iter().map(|row| row.count).sum::<usize>(), 2);
    assert!(store.health_check().await);
    assert!(store.forget("a", "one").await.unwrap());
    assert!(!store.forget("a", "one").await.unwrap());
}
