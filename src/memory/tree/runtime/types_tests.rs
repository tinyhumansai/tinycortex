//! Tests for the markdown time-tree node types.

use super::*;
use chrono::TimeZone;
use std::path::PathBuf;

#[test]
fn node_level_max_tokens() {
    assert_eq!(NodeLevel::Hour.max_tokens(), 1_000);
    assert_eq!(NodeLevel::Day.max_tokens(), 2_000);
    assert_eq!(NodeLevel::Month.max_tokens(), 4_000);
    assert_eq!(NodeLevel::Year.max_tokens(), 8_000);
    assert_eq!(NodeLevel::Root.max_tokens(), 20_000);
}

#[test]
fn node_level_parent_chain() {
    assert_eq!(NodeLevel::Hour.parent_level(), Some(NodeLevel::Day));
    assert_eq!(NodeLevel::Day.parent_level(), Some(NodeLevel::Month));
    assert_eq!(NodeLevel::Month.parent_level(), Some(NodeLevel::Year));
    assert_eq!(NodeLevel::Year.parent_level(), Some(NodeLevel::Root));
    assert_eq!(NodeLevel::Root.parent_level(), None);
}

#[test]
fn derive_parent_id_chain() {
    assert_eq!(derive_parent_id("2024/03/15/14"), Some("2024/03/15".into()));
    assert_eq!(derive_parent_id("2024/03/15"), Some("2024/03".into()));
    assert_eq!(derive_parent_id("2024/03"), Some("2024".into()));
    assert_eq!(derive_parent_id("2024"), Some("root".into()));
    assert_eq!(derive_parent_id("root"), None);
}

#[test]
fn level_from_node_id_all_levels() {
    assert_eq!(level_from_node_id("root"), NodeLevel::Root);
    assert_eq!(level_from_node_id("2024"), NodeLevel::Year);
    assert_eq!(level_from_node_id("2024/03"), NodeLevel::Month);
    assert_eq!(level_from_node_id("2024/03/15"), NodeLevel::Day);
    assert_eq!(level_from_node_id("2024/03/15/14"), NodeLevel::Hour);
}

#[test]
fn derive_node_ids_from_timestamp() {
    let ts = Utc.with_ymd_and_hms(2024, 3, 15, 14, 30, 0).unwrap();
    let (hour, day, month, year, root) = derive_node_ids(&ts);
    assert_eq!(hour, "2024/03/15/14");
    assert_eq!(day, "2024/03/15");
    assert_eq!(month, "2024/03");
    assert_eq!(year, "2024");
    assert_eq!(root, "root");
}

#[test]
fn node_id_to_path_mapping() {
    assert_eq!(node_id_to_path("root"), PathBuf::from("root.md"));
    assert_eq!(node_id_to_path("2024"), PathBuf::from("2024/summary.md"));
    assert_eq!(
        node_id_to_path("2024/03"),
        PathBuf::from("2024/03/summary.md")
    );
    assert_eq!(
        node_id_to_path("2024/03/15/14"),
        PathBuf::from("2024/03/15/14.md")
    );
}

#[test]
fn estimate_tokens_rough() {
    assert_eq!(estimate_tokens(""), 0);
    assert_eq!(estimate_tokens("abcd"), 1);
    assert_eq!(estimate_tokens(&"a".repeat(4000)), 1000);
}

#[test]
fn node_level_roundtrip() {
    for level in [
        NodeLevel::Root,
        NodeLevel::Year,
        NodeLevel::Month,
        NodeLevel::Day,
        NodeLevel::Hour,
    ] {
        assert_eq!(NodeLevel::from_str_label(level.as_str()), Some(level));
    }
}
