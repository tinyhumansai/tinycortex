use super::*;

#[test]
fn search_by_vector_scores_correct() {
    let store = fake_store(3);
    store
        .insert_with_vector("x", "ns", "x", &[1.0, 0.0, 0.0], json!({}))
        .unwrap();
    store
        .insert_with_vector("y", "ns", "y", &[0.0, 1.0, 0.0], json!({}))
        .unwrap();
    let results = store.search_by_vector("ns", &[1.0, 0.0, 0.0], 2).unwrap();
    assert_eq!(results[0].id, "x");
    assert!((results[0].score - 1.0).abs() < 1e-6);
    assert!(results[1].score < 1e-6);
}

#[test]
fn search_by_vector_preserves_metadata() {
    let store = fake_store(2);
    store
        .insert_with_vector("a", "ns", "t", &[1.0, 0.0], json!({"key": "value"}))
        .unwrap();
    assert_eq!(
        store.search_by_vector("ns", &[1.0, 0.0], 1).unwrap()[0].metadata["key"],
        "value"
    );
}

#[test]
fn search_handles_invalid_metadata_json() {
    let store = fake_store(2);
    {
        let conn = store.conn.lock();
        conn.execute(
            "INSERT INTO vectors (id, namespace, text, embedding, metadata, created_at, updated_at)
             VALUES ('bad', 'ns', 'text', ?1, 'not-json', 0.0, 0.0)",
            rusqlite::params![vec_to_bytes(&[1.0, 0.0])],
        )
        .unwrap();
    }
    let results = store.search_by_vector("ns", &[1.0, 0.0], 1).unwrap();
    assert_eq!(results[0].id, "bad");
    assert!(results[0].metadata.is_null());
}

// ── delete ──────────────────────────────────────────────

#[tokio::test]
async fn delete_existing() {
    let store = fake_store(4);
    store.insert("a", "ns", "text", json!({})).await.unwrap();
    assert!(store.delete("ns", "a").unwrap());
    assert_eq!(store.count(Some("ns")).unwrap(), 0);
}

#[test]
fn delete_nonexistent() {
    assert!(!fake_store(3).delete("ns", "no-such-id").unwrap());
}

#[tokio::test]
async fn delete_wrong_namespace() {
    let store = fake_store(4);
    store.insert("a", "ns1", "text", json!({})).await.unwrap();
    assert!(!store.delete("ns2", "a").unwrap());
    assert_eq!(store.count(Some("ns1")).unwrap(), 1);
}

// ── clear_namespace ─────────────────────────────────────

#[tokio::test]
async fn clear_namespace_removes_all() {
    let store = fake_store(4);
    store.insert("a", "ns", "one", json!({})).await.unwrap();
    store.insert("b", "ns", "two", json!({})).await.unwrap();
    store
        .insert("c", "other", "three", json!({}))
        .await
        .unwrap();
    assert_eq!(store.clear_namespace("ns").unwrap(), 2);
    assert_eq!(store.count(Some("ns")).unwrap(), 0);
    assert_eq!(store.count(Some("other")).unwrap(), 1);
}

#[test]
fn clear_empty_namespace() {
    assert_eq!(fake_store(3).clear_namespace("empty").unwrap(), 0);
}

// ── list_namespaces ─────────────────────────────────────

#[tokio::test]
async fn list_namespaces_empty() {
    assert!(fake_store(3).list_namespaces().unwrap().is_empty());
}

#[tokio::test]
async fn list_namespaces_populated() {
    let store = fake_store(4);
    store.insert("a", "beta", "t", json!({})).await.unwrap();
    store.insert("b", "alpha", "t", json!({})).await.unwrap();
    store.insert("c", "beta", "t", json!({})).await.unwrap();
    assert_eq!(store.list_namespaces().unwrap(), vec!["alpha", "beta"]);
}

// ── count ───────────────────────────────────────────────

#[test]
fn count_empty() {
    let store = fake_store(3);
    assert_eq!(store.count(None).unwrap(), 0);
    assert_eq!(store.count(Some("ns")).unwrap(), 0);
}
