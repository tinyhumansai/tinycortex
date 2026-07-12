//! Tests for the markdown time-tree engine. Adapted from OpenHuman's
//! `tree_runtime/engine_tests.rs`: the chat `Provider` becomes the [`Summariser`]
//! trait and the `model` argument is dropped.

use super::*;
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use std::sync::Mutex;
use tempfile::TempDir;

use crate::memory::config::MemoryConfig;
use crate::memory::tree::runtime::types::{derive_parent_id, estimate_tokens, NodeLevel, TreeNode};

fn test_config(tmp: &TempDir) -> MemoryConfig {
    MemoryConfig::new(tmp.path().join("workspace"))
}

/// Summariser returning a fixed reply.
struct StubSummariser {
    reply: String,
}

#[derive(Default)]
struct RecordingObserver {
    events: Mutex<Vec<String>>,
}

impl RuntimeObserver for RecordingObserver {
    fn hour_completed(&self, namespace: &str, node_id: &str, _token_count: u32) {
        self.events
            .lock()
            .unwrap()
            .push(format!("hour:{namespace}:{node_id}"));
    }

    fn node_propagated(
        &self,
        namespace: &str,
        node_id: &str,
        _level: NodeLevel,
        _token_count: u32,
    ) {
        self.events
            .lock()
            .unwrap()
            .push(format!("propagated:{namespace}:{node_id}"));
    }
}
impl StubSummariser {
    fn with_reply(reply: impl Into<String>) -> Self {
        Self {
            reply: reply.into(),
        }
    }
}
#[async_trait]
impl Summariser for StubSummariser {
    async fn summarise(&self, _system: Option<&str>, _content: &str) -> anyhow::Result<String> {
        Ok(self.reply.clone())
    }
}

/// Summariser that errors only at one summarisation level.
struct FailAtLevelSummariser {
    fail_level: &'static str,
    reply: String,
}
#[async_trait]
impl Summariser for FailAtLevelSummariser {
    async fn summarise(&self, system: Option<&str>, _content: &str) -> anyhow::Result<String> {
        if system
            .unwrap_or("")
            .contains(&format!("at the {} level", self.fail_level))
        {
            anyhow::bail!("simulated {} summarization failure", self.fail_level);
        }
        Ok(self.reply.clone())
    }
}

fn hour_node(ns: &str, id: &str, summary: &str, ts: chrono::DateTime<Utc>) -> TreeNode {
    TreeNode {
        node_id: id.to_string(),
        namespace: ns.to_string(),
        level: NodeLevel::Hour,
        parent_id: derive_parent_id(id),
        summary: summary.to_string(),
        token_count: estimate_tokens(summary),
        child_count: 0,
        created_at: ts,
        updated_at: ts,
        metadata: None,
    }
}

#[test]
fn group_by_hour_buckets_entries() {
    assert!(group_by_hour(&[]).is_empty());
    let ts_a = 1_711_958_400_000_i64; // 2024-04-01T08:00:00Z
    let entries = vec![
        (format!("{ts_a}_uuid1.md"), "msg-a".to_string()),
        (
            format!("{}_uuid2.md", ts_a + 1_800_000),
            "msg-b".to_string(),
        ),
        (
            format!("{}_uuid3.md", 1_711_962_000_000_i64),
            "hour9".to_string(),
        ),
    ];
    let groups = group_by_hour(&entries);
    assert_eq!(groups.len(), 2);
    let keys: Vec<&String> = groups.keys().collect();
    assert!(keys[0].ends_with("/08"));
    assert!(keys[1].ends_with("/09"));
    assert_eq!(groups.values().next().unwrap().len(), 2);
}

#[test]
fn group_by_hour_unparseable_falls_back() {
    let groups = group_by_hour(&[("bad-filename.md".to_string(), "x".to_string())]);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups.keys().next().unwrap().matches('/').count(), 3);
}

#[tokio::test]
async fn propagate_node_with_no_children_is_noop() {
    let tmp = TempDir::new().unwrap();
    let cfg = test_config(&tmp);
    let s = StubSummariser::with_reply("unused");
    propagate_node(&cfg, &s, "test-ns", "2024/03/15", NodeLevel::Day)
        .await
        .unwrap();
    assert!(store::read_node(&cfg, "test-ns", "2024/03/15")
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn propagate_node_day_from_hour_children_fits_budget() {
    let tmp = TempDir::new().unwrap();
    let cfg = test_config(&tmp);
    let now = Utc.with_ymd_and_hms(2024, 3, 15, 8, 0, 0).unwrap();
    store::write_node(
        &cfg,
        &hour_node("test-ns", "2024/03/15/08", "Meeting at 8am.", now),
    )
    .unwrap();
    store::write_node(
        &cfg,
        &hour_node("test-ns", "2024/03/15/09", "Stand-up at 9am.", now),
    )
    .unwrap();

    let s = StubSummariser::with_reply("SHOULD_NOT_APPEAR");
    propagate_node(&cfg, &s, "test-ns", "2024/03/15", NodeLevel::Day)
        .await
        .unwrap();
    let day = store::read_node(&cfg, "test-ns", "2024/03/15")
        .unwrap()
        .unwrap();
    assert_eq!(day.level, NodeLevel::Day);
    assert!(day.summary.contains("Meeting at 8am."));
    assert!(day.summary.contains("Stand-up at 9am."));
    assert!(!day.summary.contains("SHOULD_NOT_APPEAR"));
    assert!(day.child_count >= 2);
}

#[tokio::test]
async fn propagate_node_preserves_created_at_on_update() {
    let tmp = TempDir::new().unwrap();
    let cfg = test_config(&tmp);
    let now = Utc::now();
    let mut existing = hour_node(
        "test-ns",
        "2024/03/15",
        "old",
        now - chrono::Duration::hours(5),
    );
    existing.level = NodeLevel::Day;
    existing.parent_id = Some("2024/03".into());
    store::write_node(&cfg, &existing).unwrap();
    let original_created = existing.created_at;
    store::write_node(
        &cfg,
        &hour_node("test-ns", "2024/03/15/10", "hour content", now),
    )
    .unwrap();

    let s = StubSummariser::with_reply("updated summary");
    propagate_node(&cfg, &s, "test-ns", "2024/03/15", NodeLevel::Day)
        .await
        .unwrap();
    let updated = store::read_node(&cfg, "test-ns", "2024/03/15")
        .unwrap()
        .unwrap();
    assert_eq!(updated.created_at, original_created);
    assert!(updated.updated_at >= now);
}

#[tokio::test]
async fn run_summarization_empty_buffer_returns_none() {
    let tmp = TempDir::new().unwrap();
    let cfg = test_config(&tmp);
    let s = StubSummariser::with_reply("unused");
    assert!(run_summarization(&cfg, &s, "test-ns", Utc::now())
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn run_summarization_builds_ancestor_chain() {
    let tmp = TempDir::new().unwrap();
    let cfg = test_config(&tmp);
    let ns = "ancestor-test";
    let ts = Utc.with_ymd_and_hms(2024, 3, 15, 10, 0, 0).unwrap();
    store::buffer_write(&cfg, ns, "test content", &ts, None).unwrap();

    let s = StubSummariser::with_reply("summary text");
    let last = run_summarization(&cfg, &s, ns, ts).await.unwrap().unwrap();
    assert_eq!(last.level, NodeLevel::Hour);
    for id in ["2024/03/15", "2024/03", "2024", "root"] {
        assert!(
            store::read_node(&cfg, ns, id).unwrap().is_some(),
            "missing {id}"
        );
    }
    assert!(store::buffer_read(&cfg, ns).unwrap().is_empty());
}

#[tokio::test]
async fn observed_run_reports_hour_and_propagation_progress() {
    let tmp = TempDir::new().unwrap();
    let cfg = test_config(&tmp);
    let ns = "observed";
    let ts = Utc.with_ymd_and_hms(2024, 4, 1, 8, 0, 0).unwrap();
    store::buffer_write(&cfg, ns, "raw", &ts, None).unwrap();
    let observer = RecordingObserver::default();

    run_summarization_observed(
        &cfg,
        &StubSummariser::with_reply("summary"),
        ns,
        ts,
        &observer,
    )
    .await
    .unwrap();

    let events = observer.events.lock().unwrap();
    assert!(events
        .iter()
        .any(|event| event.starts_with("hour:observed:")));
    assert!(events
        .iter()
        .any(|event| event == "propagated:observed:root"));
}

#[tokio::test]
async fn run_summarization_multi_hour_groups_produce_multiple_leaves() {
    let tmp = TempDir::new().unwrap();
    let cfg = test_config(&tmp);
    let ns = "multi-hour";
    store::buffer_write(
        &cfg,
        ns,
        "morning",
        &Utc.with_ymd_and_hms(2024, 3, 15, 8, 0, 0).unwrap(),
        None,
    )
    .unwrap();
    store::buffer_write(
        &cfg,
        ns,
        "afternoon",
        &Utc.with_ymd_and_hms(2024, 3, 15, 14, 0, 0).unwrap(),
        None,
    )
    .unwrap();
    let s = StubSummariser::with_reply("grouped");
    run_summarization(&cfg, &s, ns, Utc::now()).await.unwrap();
    assert!(store::read_node(&cfg, ns, "2024/03/15/08")
        .unwrap()
        .is_some());
    assert!(store::read_node(&cfg, ns, "2024/03/15/14")
        .unwrap()
        .is_some());
    assert!(store::buffer_read(&cfg, ns).unwrap().is_empty());
}

#[tokio::test]
async fn rebuild_tree_on_empty_namespace_is_noop() {
    let tmp = TempDir::new().unwrap();
    let cfg = test_config(&tmp);
    let s = StubSummariser::with_reply("unused");
    let status = rebuild_tree(&cfg, &s, "empty").await.unwrap();
    assert_eq!(status.total_nodes, 0);
    assert_eq!(status.depth, 0);
}

#[tokio::test]
async fn rebuild_tree_rewrites_ancestors() {
    let tmp = TempDir::new().unwrap();
    let cfg = test_config(&tmp);
    let ns = "rebuild";
    let ts = Utc.with_ymd_and_hms(2024, 3, 15, 10, 0, 0).unwrap();
    store::write_node(&cfg, &hour_node(ns, "2024/03/15/10", "hour ten", ts)).unwrap();
    store::write_node(&cfg, &hour_node(ns, "2024/03/15/11", "hour eleven", ts)).unwrap();
    store::buffer_write(&cfg, ns, "pending", &ts, None).unwrap();

    let s = StubSummariser::with_reply("rebuilt summary");
    let status = rebuild_tree(&cfg, &s, ns).await.unwrap();
    assert!(status.total_nodes >= 5);
    assert_eq!(store::buffer_read(&cfg, ns).unwrap().len(), 1);
    assert!(store::read_node(&cfg, ns, "root").unwrap().is_some());
}

#[tokio::test]
async fn rebuild_tree_partial_success_when_one_level_fails() {
    let tmp = TempDir::new().unwrap();
    let cfg = test_config(&tmp);
    let ns = "partial-rebuild";
    let ts = Utc.with_ymd_and_hms(2024, 3, 15, 10, 0, 0).unwrap();
    let big = "word ".repeat(1000); // ~1250 tokens each → day combine busts 2000 budget
    store::write_node(&cfg, &hour_node(ns, "2024/03/15/10", &big, ts)).unwrap();
    store::write_node(&cfg, &hour_node(ns, "2024/03/15/11", &big, ts)).unwrap();

    let s = FailAtLevelSummariser {
        fail_level: "day",
        reply: "ok".to_string(),
    };
    let status = rebuild_tree(&cfg, &s, ns).await.unwrap();
    assert!(status.total_nodes >= 2);
    assert!(store::read_node(&cfg, ns, "2024/03/15/10")
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn summarize_to_limit_truncates_overlong_output() {
    let s = StubSummariser::with_reply("x".repeat(MAX_SUMMARY_CHARS + 50));
    let summary = summarize_to_limit(&s, "short", 10, "day", "2024/03/15")
        .await
        .unwrap();
    assert_eq!(summary.len(), 40);
    assert!(summary.chars().all(|c| c == 'x'));
}

#[test]
fn discover_active_namespaces_requires_markdown_entries() {
    let tmp = TempDir::new().unwrap();
    let cfg = test_config(&tmp);
    let base = cfg.workspace.join("memory").join("namespaces");
    std::fs::create_dir_all(base.join("alpha").join("tree").join("buffer")).unwrap();
    std::fs::create_dir_all(base.join("beta").join("tree").join("buffer")).unwrap();
    std::fs::write(
        base.join("alpha")
            .join("tree")
            .join("buffer")
            .join("entry.md"),
        "a",
    )
    .unwrap();
    std::fs::write(
        base.join("beta")
            .join("tree")
            .join("buffer")
            .join("entry.txt"),
        "b",
    )
    .unwrap();
    assert_eq!(discover_active_namespaces(&cfg), vec!["alpha".to_string()]);
}
