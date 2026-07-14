//! Tests for the markdown time-tree store. Adapted from OpenHuman's
//! `tree_runtime/store_tests.rs`: `Config` → [`MemoryConfig`], `workspace_dir`
//! → `workspace`.

use super::*;
use chrono::{DateTime, TimeZone, Utc};
use tempfile::TempDir;

use crate::memory::config::MemoryConfig;
use crate::memory::tree::runtime::types::*;

fn test_config(tmp: &TempDir) -> MemoryConfig {
    MemoryConfig::new(tmp.path().join("workspace"))
}

fn make_node(namespace: &str, node_id: &str, summary: &str) -> TreeNode {
    TreeNode {
        node_id: node_id.to_string(),
        namespace: namespace.to_string(),
        level: level_from_node_id(node_id),
        parent_id: derive_parent_id(node_id),
        summary: summary.to_string(),
        token_count: estimate_tokens(summary),
        child_count: 0,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        metadata: None,
    }
}

#[test]
fn write_and_read_node_roundtrip() {
    let tmp = TempDir::new().unwrap();
    let config = test_config(&tmp);
    let node = make_node("test-ns", "root", "All-time summary of events.");
    write_node(&config, &node).unwrap();
    let read_back = read_node(&config, "test-ns", "root").unwrap().unwrap();
    assert_eq!(read_back.node_id, "root");
    assert_eq!(read_back.level, NodeLevel::Root);
    assert_eq!(read_back.summary, "All-time summary of events.");
    assert!(read_back.parent_id.is_none());
}

#[test]
fn node_frontmatter_strings_escape_newlines_and_delimiters() {
    let tmp = TempDir::new().unwrap();
    let config = test_config(&tmp);
    let mut node = make_node("namespace\n---\nforged: true", "2024", "summary");
    node.metadata = Some("line one\n---\nnode_id: forged\\tail".into());
    write_node(&config, &node).unwrap();

    let read = read_node(&config, &node.namespace, &node.node_id)
        .unwrap()
        .unwrap();
    assert_eq!(read.namespace, node.namespace);
    assert_eq!(read.metadata, node.metadata);
    assert_eq!(read.summary, node.summary);
}

#[test]
fn write_and_read_hour_leaf() {
    let tmp = TempDir::new().unwrap();
    let config = test_config(&tmp);
    write_node(
        &config,
        &make_node("test-ns", "2024/03/15/14", "Hour 14 summary."),
    )
    .unwrap();
    let read_back = read_node(&config, "test-ns", "2024/03/15/14")
        .unwrap()
        .unwrap();
    assert_eq!(read_back.level, NodeLevel::Hour);
    assert_eq!(read_back.parent_id.as_deref(), Some("2024/03/15"));
}

#[test]
fn read_children_of_day() {
    let tmp = TempDir::new().unwrap();
    let config = test_config(&tmp);
    for hour in [10, 11, 14] {
        write_node(
            &config,
            &make_node(
                "test-ns",
                &format!("2024/03/15/{hour:02}"),
                &format!("Hour {hour}."),
            ),
        )
        .unwrap();
    }
    write_node(&config, &make_node("test-ns", "2024/03/15", "Day summary.")).unwrap();
    let children = read_children(&config, "test-ns", "2024/03/15").unwrap();
    assert_eq!(children.len(), 3);
    assert_eq!(children[0].node_id, "2024/03/15/10");
    assert_eq!(children[2].node_id, "2024/03/15/14");
}

#[test]
fn read_children_of_root() {
    let tmp = TempDir::new().unwrap();
    let config = test_config(&tmp);
    for year in ["2023", "2024"] {
        write_node(
            &config,
            &make_node("test-ns", year, &format!("Year {year}.")),
        )
        .unwrap();
    }
    let children = read_children(&config, "test-ns", "root").unwrap();
    assert_eq!(children.len(), 2);
    assert_eq!(children[0].node_id, "2023");
    assert_eq!(children[1].node_id, "2024");
}

#[test]
fn read_node_missing_returns_none() {
    let tmp = TempDir::new().unwrap();
    let config = test_config(&tmp);
    assert!(read_node(&config, "ns", "root").unwrap().is_none());
}

#[test]
fn count_nodes_and_status() {
    let tmp = TempDir::new().unwrap();
    let config = test_config(&tmp);
    for (id, s) in [
        ("root", "root"),
        ("2024", "year"),
        ("2024/03", "month"),
        ("2024/03/15", "day"),
        ("2024/03/15/14", "hour"),
    ] {
        write_node(&config, &make_node("test-ns", id, s)).unwrap();
    }
    assert_eq!(count_nodes(&config, "test-ns").unwrap(), 5);
    let status = get_tree_status(&config, "test-ns").unwrap();
    assert_eq!(status.total_nodes, 5);
    assert_eq!(status.depth, 5);
}

#[test]
fn delete_tree_removes_all() {
    let tmp = TempDir::new().unwrap();
    let config = test_config(&tmp);
    write_node(&config, &make_node("test-ns", "root", "root")).unwrap();
    write_node(&config, &make_node("test-ns", "2024/03/15/14", "hour")).unwrap();
    let deleted = delete_tree(&config, "test-ns").unwrap();
    assert!(deleted >= 2);
    assert_eq!(count_nodes(&config, "test-ns").unwrap(), 0);
}

#[test]
fn buffer_write_and_drain() {
    let tmp = TempDir::new().unwrap();
    let config = test_config(&tmp);
    let ts1 = Utc.with_ymd_and_hms(2024, 3, 15, 10, 0, 0).unwrap();
    let ts2 = Utc.with_ymd_and_hms(2024, 3, 15, 11, 0, 0).unwrap();
    buffer_write(&config, "test-ns", "entry one", &ts1, None).unwrap();
    buffer_write(&config, "test-ns", "entry two", &ts2, None).unwrap();
    let drained = buffer_drain(&config, "test-ns").unwrap();
    assert_eq!(drained.len(), 2);
    assert_eq!(drained[0].1, "entry one");
    assert_eq!(drained[1].1, "entry two");
    assert!(buffer_drain(&config, "test-ns").unwrap().is_empty());
}

#[test]
fn buffer_write_with_metadata() {
    let tmp = TempDir::new().unwrap();
    let config = test_config(&tmp);
    let meta = serde_json::json!({"source": "test", "priority": 1});
    buffer_write(
        &config,
        "test-ns",
        "entry with meta",
        &Utc::now(),
        Some(&meta),
    )
    .unwrap();
    let drained = buffer_drain(&config, "test-ns").unwrap();
    assert_eq!(drained.len(), 1);
    assert_eq!(drained[0].1, "entry with meta");
}

#[test]
fn buffer_content_starting_with_horizontal_rule_is_not_truncated() {
    let tmp = TempDir::new().unwrap();
    let config = test_config(&tmp);
    let content = "---\nuser-authored heading\n---\nbody";
    buffer_write(&config, "test-ns", content, &Utc::now(), None).unwrap();
    let drained = buffer_drain(&config, "test-ns").unwrap();
    assert_eq!(drained[0].1, content);
}

#[test]
fn ancestors_walk_to_root() {
    let tmp = TempDir::new().unwrap();
    let config = test_config(&tmp);
    for (id, s) in [
        ("root", "root"),
        ("2024", "year"),
        ("2024/03", "month"),
        ("2024/03/15", "day"),
    ] {
        write_node(&config, &make_node("test-ns", id, s)).unwrap();
    }
    let ancestors = read_ancestors(&config, "test-ns", "2024/03/15/14").unwrap();
    let ids: Vec<&str> = ancestors.iter().map(|n| n.node_id.as_str()).collect();
    assert_eq!(ids, vec!["2024/03/15", "2024/03", "2024", "root"]);
}

#[test]
fn frontmatter_parsing() {
    let raw = "---\nnode_id: \"root\"\nlevel: root\ntoken_count: 42\n---\n\nHello world.";
    let (fm, body) = split_frontmatter(raw);
    assert_eq!(fm.get("level").unwrap(), "root");
    assert_eq!(fm.get("token_count").unwrap(), "42");
    assert_eq!(body, "Hello world.");
}

#[test]
fn parse_node_markdown_uses_deterministic_fallback_timestamps() {
    let raw = "---\nnode_id: \"root\"\nlevel: root\n---\n\nUndated summary.";
    let node = parse_node_markdown_pub(raw, "ns", "root").unwrap();
    assert_eq!(node.created_at, DateTime::<Utc>::UNIX_EPOCH);
    assert_eq!(node.updated_at, DateTime::<Utc>::UNIX_EPOCH);

    let raw =
        "---\nnode_id: \"root\"\nlevel: root\ncreated_at: 2026-05-25T09:00:00Z\n---\n\nSummary.";
    let node = parse_node_markdown_pub(raw, "ns", "root").unwrap();
    let created = DateTime::parse_from_rfc3339("2026-05-25T09:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    assert_eq!(node.created_at, created);
    assert_eq!(node.updated_at, created);
}

#[test]
fn validate_node_id_accepts_valid_and_rejects_bad() {
    for ok in ["root", "2024", "2024/03", "2024/03/15", "2024/03/15/14"] {
        assert!(validate_node_id(ok).is_ok());
    }
    for bad in [
        "..",
        "../etc",
        "2024/../etc",
        "/2024",
        "2024/",
        "abc",
        "2024/abc",
        "2024/13",
        "2024/03/32",
        "2024/03/15/24",
    ] {
        assert!(validate_node_id(bad).is_err(), "{bad} should be rejected");
    }
}

#[test]
fn validate_namespace_accepts_and_rejects() {
    assert!(validate_namespace("my-namespace").is_ok());
    assert!(validate_namespace("skill:gmail:user@example.com").is_ok());
    for bad in ["", "  ", "../etc", "/absolute"] {
        assert!(validate_namespace(bad).is_err());
    }
}

#[test]
fn transformed_namespace_paths_are_collision_resistant() {
    let tmp = TempDir::new().unwrap();
    let cfg = test_config(&tmp);
    assert_ne!(tree_dir(&cfg, "a/b"), tree_dir(&cfg, "a.b"));
}

#[test]
fn list_namespaces_with_root_returns_only_summarised() {
    let tmp = TempDir::new().unwrap();
    let config = test_config(&tmp);
    write_node(&config, &make_node("ns_a", "root", "alpha summary")).unwrap();
    write_node(&config, &make_node("ns_b", "2024/03/15/14", "hour")).unwrap();
    write_node(&config, &make_node("ns_c", "root", "gamma summary")).unwrap();
    assert_eq!(
        list_namespaces_with_root(&config).unwrap(),
        vec!["ns_a".to_string(), "ns_c".to_string()]
    );
}

#[test]
fn collect_root_summaries_respects_per_namespace_cap() {
    let tmp = TempDir::new().unwrap();
    let config = test_config(&tmp);
    write_node(&config, &make_node("ns", "root", &"x".repeat(50))).unwrap();
    let result = collect_root_summaries_with_caps(&config.workspace, 10, 10_000);
    assert_eq!(result.len(), 1);
    assert!(result[0].1.starts_with("xxxxxxxxxx"));
    assert!(result[0].1.contains("[... truncated]"));
}

#[test]
fn collect_root_summaries_carries_root_updated_at() {
    let tmp = TempDir::new().unwrap();
    let config = test_config(&tmp);
    let fixed = DateTime::parse_from_rfc3339("2026-05-25T09:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let mut node = make_node("ns", "root", "summary body");
    node.updated_at = fixed;
    write_node(&config, &node).unwrap();
    let result = collect_root_summaries_with_caps(&config.workspace, 1000, 10_000);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].2, fixed);
}

#[test]
fn collect_root_summaries_stops_at_total_cap() {
    let tmp = TempDir::new().unwrap();
    let config = test_config(&tmp);
    write_node(&config, &make_node("aaa", "root", "first")).unwrap();
    write_node(&config, &make_node("bbb", "root", "second")).unwrap();
    write_node(&config, &make_node("ccc", "root", "third")).unwrap();
    let result = collect_root_summaries_with_caps(&config.workspace, 100, 5);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].0, "aaa");
}

#[test]
fn collect_root_summaries_returns_empty_for_unknown_workspace() {
    let tmp = TempDir::new().unwrap();
    assert!(collect_root_summaries_with_caps(&tmp.path().join("nope"), 100, 1000).is_empty());
}

#[test]
fn write_node_overwrite_is_atomic_and_replaces_content() {
    let tmp = TempDir::new().unwrap();
    let config = test_config(&tmp);
    let path = node_file_path(&config, "test-ns", "2024/03/15/14");

    write_node(
        &config,
        &make_node("test-ns", "2024/03/15/14", "old summary"),
    )
    .unwrap();
    write_node(
        &config,
        &make_node("test-ns", "2024/03/15/14", "new summary"),
    )
    .unwrap();

    // The node file holds the fully-replaced new content (old-or-new, never torn).
    let read_back = read_node(&config, "test-ns", "2024/03/15/14")
        .unwrap()
        .unwrap();
    assert_eq!(read_back.summary, "new summary");

    // The atomic temp file must have been renamed away, not left behind.
    let dir = path.parent().unwrap();
    let leftovers: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .filter(|name| name != "14.md")
        .collect();
    assert!(
        leftovers.is_empty(),
        "write_node must not leave temp files behind, found: {leftovers:?}"
    );
}

#[test]
fn read_children_ignores_stray_temp_files() {
    let tmp = TempDir::new().unwrap();
    let config = test_config(&tmp);

    // A real hour leaf plus a stray atomic-write temp file in the same day dir.
    write_node(&config, &make_node("test-ns", "2024/03/15/14", "hour 14")).unwrap();
    let day_dir = node_file_path(&config, "test-ns", "2024/03/15/14")
        .parent()
        .unwrap()
        .to_path_buf();
    std::fs::write(day_dir.join(".14.md.tmp-deadbeef"), b"partial junk").unwrap();

    // The stray temp file must not be parsed as a node.
    let children = read_children(&config, "test-ns", "2024/03/15").unwrap();
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].node_id, "2024/03/15/14");
}
