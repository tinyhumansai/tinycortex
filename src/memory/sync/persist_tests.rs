//! Unit tests for [`KvSkillDocSink`].

use std::sync::Arc;

use serde_json::json;

use super::*;
use crate::memory::store::KvStore;
use crate::memory::sync::traits::SkillDocument;

fn doc(toolkit: &str, id: &str, content: &str) -> SkillDocument {
    SkillDocument {
        namespace_skill_id: toolkit.into(),
        connection_id: "conn_123".into(),
        document_id: format!("{toolkit}:{id}"),
        title: format!("doc {id}"),
        content: content.into(),
        toolkit: toolkit.into(),
        metadata: json!({ "taint": "external_sync", "provider_id": id }),
    }
}

fn sink() -> (KvSkillDocSink, Arc<KvStore>) {
    let kv = Arc::new(KvStore::open_in_memory().expect("open kv"));
    (KvSkillDocSink::new(kv.clone()), kv)
}

#[tokio::test]
async fn store_persists_document_under_toolkit_namespace() {
    let (sink, kv) = sink();
    sink.store(doc("gmail", "abc", "hello world"))
        .await
        .expect("store");

    let stored = kv
        .get_namespace(&KvSkillDocSink::namespace_for("gmail"), "gmail:abc")
        .expect("read")
        .expect("present");
    assert_eq!(stored["title"], "doc abc");
    assert_eq!(stored["toolkit"], "gmail");
    assert_eq!(stored["content"], "hello world");
    assert_eq!(stored["metadata"]["taint"], "external_sync");
}

#[tokio::test]
async fn documents_are_grouped_by_toolkit() {
    let (sink, kv) = sink();
    sink.store(doc("gmail", "a", "one")).await.unwrap();
    sink.store(doc("gmail", "b", "two")).await.unwrap();
    sink.store(doc("github", "c", "three")).await.unwrap();

    let gmail = kv
        .records_namespace(&KvSkillDocSink::namespace_for("gmail"))
        .expect("list gmail");
    let github = kv
        .records_namespace(&KvSkillDocSink::namespace_for("github"))
        .expect("list github");
    assert_eq!(gmail.len(), 2);
    assert_eq!(github.len(), 1);
}

#[tokio::test]
async fn store_is_idempotent_upsert() {
    let (sink, kv) = sink();
    sink.store(doc("gmail", "a", "v1")).await.unwrap();
    sink.store(doc("gmail", "a", "v2")).await.unwrap();

    let records = kv
        .records_namespace(&KvSkillDocSink::namespace_for("gmail"))
        .expect("list");
    assert_eq!(records.len(), 1, "same document_id upserts in place");
    let stored = kv
        .get_namespace(&KvSkillDocSink::namespace_for("gmail"), "gmail:a")
        .unwrap()
        .unwrap();
    assert_eq!(stored["content"], "v2");
}

#[tokio::test]
async fn delete_removes_only_the_target() {
    let (sink, kv) = sink();
    sink.store(doc("gmail", "a", "one")).await.unwrap();
    sink.store(doc("gmail", "b", "two")).await.unwrap();

    sink.delete("gmail", "gmail:a").await.expect("delete");

    assert!(kv
        .get_namespace(&KvSkillDocSink::namespace_for("gmail"), "gmail:a")
        .unwrap()
        .is_none());
    assert!(kv
        .get_namespace(&KvSkillDocSink::namespace_for("gmail"), "gmail:b")
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn stored_value_is_sanitized() {
    // The KV layer scrubs secrets from the persisted value, so a debug viewer
    // over this store never surfaces raw credentials.
    let (sink, kv) = sink();
    sink.store(doc(
        "gmail",
        "secretful",
        "token sk-ABCDEF0123456789ABCDEF0123456789 end",
    ))
    .await
    .unwrap();

    let stored = kv
        .get_namespace(&KvSkillDocSink::namespace_for("gmail"), "gmail:secretful")
        .unwrap()
        .unwrap();
    let content = stored["content"].as_str().unwrap();
    assert!(
        !content.contains("sk-ABCDEF0123456789ABCDEF0123456789"),
        "raw secret should be redacted before persistence, got: {content}"
    );
}
