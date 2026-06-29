//! Tests for the chunk-source injection seam. The `extract_item_id` cases are
//! ported from OpenHuman `memory_diff::ops` tests.

use super::*;
use crate::memory::diff::types::SnapshotItem;

#[test]
fn extract_item_id_reader_backed() {
    assert_eq!(extract_item_id("mem_src:src_abc:readme.md"), "readme.md");
    assert_eq!(
        extract_item_id("mem_src:src_abc:path/to/file.md"),
        "path/to/file.md"
    );
}

#[test]
fn extract_item_id_composio() {
    assert_eq!(
        extract_item_id("gmail:user@example.com:msg_xxx"),
        "user@example.com:msg_xxx"
    );
}

#[test]
fn extract_item_id_no_prefix() {
    assert_eq!(extract_item_id("standalone"), "standalone");
}

#[test]
fn in_memory_source_groups_and_orders_items() {
    let mut src = InMemoryItemSource::new();
    // Push out of id order; multiple chunks for `a` must concatenate in order.
    src.push_chunk("src_a", "mem_src:src_a:b", "beta");
    src.push_chunk("src_a", "mem_src:src_a:a", "alpha-1 ");
    src.push_chunk("src_a", "mem_src:src_a:a", "alpha-2");

    let items = src.items_for_source("src_a");
    assert_eq!(
        items,
        vec![
            SnapshotItem {
                item_id: "a".into(),
                content: "alpha-1 alpha-2".into(),
            },
            SnapshotItem {
                item_id: "b".into(),
                content: "beta".into(),
            },
        ]
    );
}

#[test]
fn in_memory_source_unknown_source_is_empty() {
    let src = InMemoryItemSource::new();
    assert!(src.items_for_source("nope").is_empty());
}

#[test]
fn set_source_replaces_contents() {
    let mut src = InMemoryItemSource::new();
    src.set_source("src_a", &[("a", "alpha"), ("b", "beta")]);
    assert_eq!(src.items_for_source("src_a").len(), 2);
    src.set_source("src_a", &[("a", "alpha v2")]);
    let items = src.items_for_source("src_a");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].content, "alpha v2");
}
