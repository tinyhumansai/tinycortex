use tinycortex::memory::{InMemoryMemoryStore, MemoryInput, MemoryQuery, MemoryStore};

#[tokio::test]
async fn stores_and_finds_memory() {
    let store = InMemoryMemoryStore::new();
    store
        .insert(MemoryInput::new(
            "default",
            "TinyCortex starts as a Rust memory core",
        ))
        .await
        .expect("insert memory");

    let hits = store
        .search(MemoryQuery::text("Rust memory"))
        .await
        .expect("search memory");

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].record.namespace, "default");
}
