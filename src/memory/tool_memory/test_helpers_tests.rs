//! Tests for the [`MockMemory`] test backend.

use super::*;

#[tokio::test]
async fn mock_memory_store_get_list_and_count_roundtrip() {
    let memory = MockMemory::default();
    memory
        .store(
            "tool-bash",
            "rule/1",
            "always dry run first",
            MemoryCategory::Custom("tool_memory".into()),
            Some("session-1"),
        )
        .await
        .unwrap();
    memory
        .store(
            "tool-web",
            "rule/2",
            "cite sources",
            MemoryCategory::Conversation,
            None,
        )
        .await
        .unwrap();

    let got = memory.get("tool-bash", "rule/1").await.unwrap().unwrap();
    assert_eq!(got.id, "tool-bash/rule/1");
    assert_eq!(got.content, "always dry run first");
    assert_eq!(got.namespace.as_deref(), Some("tool-bash"));
    assert_eq!(got.session_id.as_deref(), Some("session-1"));

    let scoped = memory.list(Some("tool-bash"), None, None).await.unwrap();
    assert_eq!(scoped.len(), 1);
    assert_eq!(scoped[0].key, "rule/1");

    let all = memory.list(None, None, None).await.unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(memory.count().await.unwrap(), 2);
    assert!(memory.health_check().await);
    assert_eq!(memory.name(), "mock");

    // The mock intentionally ignores category/session filters so tool
    // tests can focus on caller behavior instead of backend indexing.
    let filtered = memory
        .list(
            Some("tool-bash"),
            Some(&MemoryCategory::Core),
            Some("different-session"),
        )
        .await
        .unwrap();
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].key, "rule/1");
}

#[tokio::test]
async fn mock_memory_forget_and_namespace_summaries_track_entries() {
    let memory = MockMemory::default();
    memory
        .store("tool-bash", "rule/1", "first", MemoryCategory::Core, None)
        .await
        .unwrap();
    memory
        .store("tool-bash", "rule/2", "second", MemoryCategory::Daily, None)
        .await
        .unwrap();
    memory
        .store(
            "tool-web",
            "rule/3",
            "third",
            MemoryCategory::Conversation,
            None,
        )
        .await
        .unwrap();

    let mut summaries = memory.namespace_summaries().await.unwrap();
    summaries.sort_by(|a, b| a.namespace.cmp(&b.namespace));
    assert_eq!(summaries.len(), 2);
    assert_eq!(summaries[0].namespace, "tool-bash");
    assert_eq!(summaries[0].count, 2);
    assert_eq!(summaries[1].namespace, "tool-web");
    assert_eq!(summaries[1].count, 1);

    assert!(memory.forget("tool-bash", "rule/1").await.unwrap());
    assert!(!memory.forget("tool-bash", "missing").await.unwrap());

    let remaining = memory.list(Some("tool-bash"), None, None).await.unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].key, "rule/2");
}

#[tokio::test]
async fn mock_memory_recall_is_empty_noop() {
    let memory = MockMemory::default();
    let recalled = memory
        .recall("anything", 5, RecallOpts::default())
        .await
        .unwrap();
    assert!(recalled.is_empty());
}

#[tokio::test]
async fn mock_memory_empty_state_helpers_return_empty_values() {
    let memory = MockMemory::default();
    assert!(memory.get("missing", "rule").await.unwrap().is_none());
    assert!(memory
        .list(Some("missing"), None, None)
        .await
        .unwrap()
        .is_empty());
    assert!(memory.namespace_summaries().await.unwrap().is_empty());
    assert_eq!(memory.count().await.unwrap(), 0);
}
