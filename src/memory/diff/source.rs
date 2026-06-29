//! The chunk-source injection seam for the diff engine.
//!
//! Snapshots are built from already-ingested data, **not** by re-calling source
//! readers. In OpenHuman that data lives in `mem_tree_chunks`; the snapshot
//! query groups chunk content by item id and orders it by source id and
//! sequence. The `chunks` storage module is ported separately, so the diff
//! engine must not hard-depend on it. Instead it takes a [`SnapshotItemSource`]
//! by injection: any backend that can produce a source's items in the canonical
//! order satisfies the contract.
//!
//! The contract that an implementation must honour (matching OpenHuman's
//! snapshot capture):
//!
//! 1. Group chunk content by **item id** (see [`extract_item_id`]).
//! 2. Concatenate each item's chunk bodies in `seq_in_source` order.
//! 3. Return items sorted by item id (stable, deterministic ordering).
//!
//! A [`InMemoryItemSource`] is provided for tests and as a reference backend.

use std::collections::BTreeMap;

use super::types::SnapshotItem;

/// Supplies a source's current items for snapshotting.
///
/// Implementors read from the authoritative chunk store (or any equivalent) and
/// must return items already grouped and ordered per the module contract.
pub trait SnapshotItemSource {
    /// All current items for `source_id`, grouped by item id and ordered by
    /// item id. Returns an empty vector when the source has no ingested items.
    fn items_for_source(&self, source_id: &str) -> Vec<SnapshotItem>;
}

/// Extract the item-level id from a composite chunk `source_id`.
///
/// Mirrors OpenHuman's `extract_item_id`. The chunk store keys rows by a
/// composite id that prefixes the logical source; this strips that prefix to
/// recover the item id that snapshots and diffs key on.
///
/// - reader-backed: `mem_src:src_abc:readme.md` → `readme.md`
/// - composio:      `gmail:user@example.com:msg_xxx` → `user@example.com:msg_xxx`
/// - no prefix:     `standalone` → `standalone`
pub fn extract_item_id(composite: &str) -> String {
    if let Some(rest) = composite.strip_prefix("mem_src:") {
        // Skip the source-id segment.
        if let Some(pos) = rest.find(':') {
            return rest[pos + 1..].to_string();
        }
    }
    // Composio or other: strip the first segment.
    if let Some(pos) = composite.find(':') {
        return composite[pos + 1..].to_string();
    }
    composite.to_string()
}

/// A simple in-memory [`SnapshotItemSource`] for tests and as a reference.
///
/// Stores raw chunk rows as `(composite_source_id, content)` pairs and applies
/// the canonical grouping/ordering on demand, exactly as the chunk-store query
/// would.
#[derive(Debug, Clone, Default)]
pub struct InMemoryItemSource {
    /// Raw chunk rows keyed by the logical source id, each a list of
    /// `(composite_source_id, content)` in insertion (sequence) order.
    rows: BTreeMap<String, Vec<(String, String)>>,
}

impl InMemoryItemSource {
    /// An empty source.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a raw chunk row for `source_id`. `composite_source_id` is the
    /// chunk-store composite key (e.g. `mem_src:src_a:readme.md`); rows are
    /// kept in insertion order to model `seq_in_source`.
    pub fn push_chunk(
        &mut self,
        source_id: impl Into<String>,
        composite_source_id: impl Into<String>,
        content: impl Into<String>,
    ) {
        self.rows
            .entry(source_id.into())
            .or_default()
            .push((composite_source_id.into(), content.into()));
    }

    /// Convenience: set a single-chunk item directly by its (already extracted)
    /// item id under `source_id`. Equivalent to pushing one chunk whose
    /// composite id is `mem_src:<source_id>:<item_id>`.
    pub fn push_item(
        &mut self,
        source_id: impl Into<String>,
        item_id: impl Into<String>,
        content: impl Into<String>,
    ) {
        let source_id = source_id.into();
        let item_id = item_id.into();
        let composite = format!("mem_src:{source_id}:{item_id}");
        self.push_chunk(source_id, composite, content);
    }

    /// Replace all chunk rows for `source_id` (models a re-sync).
    pub fn set_source(&mut self, source_id: impl Into<String>, items: &[(&str, &str)]) {
        let source_id = source_id.into();
        let rows = items
            .iter()
            .map(|(item, content)| (format!("mem_src:{source_id}:{item}"), content.to_string()))
            .collect();
        self.rows.insert(source_id, rows);
    }
}

impl SnapshotItemSource for InMemoryItemSource {
    fn items_for_source(&self, source_id: &str) -> Vec<SnapshotItem> {
        let Some(rows) = self.rows.get(source_id) else {
            return Vec::new();
        };
        // Group content by item id, preserving chunk order within each item.
        let mut groups: BTreeMap<String, String> = BTreeMap::new();
        for (composite, content) in rows {
            let item_id = extract_item_id(composite);
            groups.entry(item_id).or_default().push_str(content);
        }
        groups
            .into_iter()
            .map(|(item_id, content)| SnapshotItem { item_id, content })
            .collect()
    }
}

#[cfg(test)]
#[path = "source_tests.rs"]
mod tests;
