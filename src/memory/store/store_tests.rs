use super::*;

#[tokio::test]
async fn inserts_and_retrieves_memory() {
    let store = InMemoryMemoryStore::new();
    let inserted = store
        .insert(MemoryInput::new("profile", "User prefers dark mode"))
        .await
        .expect("insert memory");

    let fetched = store.get(inserted.id).await.expect("get memory");

    assert_eq!(fetched.namespace, "profile");
    assert_eq!(fetched.content, "User prefers dark mode");
}

#[tokio::test]
async fn searches_by_namespace_and_text() {
    let store = InMemoryMemoryStore::new();
    store
        .insert(MemoryInput::new("profile", "User prefers dark mode"))
        .await
        .expect("insert profile memory");
    store
        .insert(MemoryInput::new(
            "project",
            "TinyCortex stores durable memories",
        ))
        .await
        .expect("insert project memory");

    let hits = store
        .search(MemoryQuery {
            namespace: Some("project".to_owned()),
            text: Some("durable".to_owned()),
            limit: Some(10),
        })
        .await
        .expect("search memory");

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].record.namespace, "project");
}

#[tokio::test]
async fn search_matches_individual_terms_and_ranks_by_coverage() {
    let store = InMemoryMemoryStore::new();
    store
        .insert(MemoryInput::new("profile", "dark theme"))
        .await
        .unwrap();
    store
        .insert(MemoryInput::new("profile", "dark mode theme"))
        .await
        .unwrap();
    let hits = store.search(MemoryQuery::text("dark mode")).await.unwrap();
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].score, 1.0);
    assert_eq!(hits[1].score, 0.5);
}
