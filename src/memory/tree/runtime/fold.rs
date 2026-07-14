//! Retry receipts and buffer grouping for hour-leaf summarisation.

use std::collections::BTreeMap;

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::store;
use super::types::derive_node_ids;
use crate::memory::config::MemoryConfig;

#[derive(Debug, Default)]
pub(super) struct BufferedHour {
    pub entries: Vec<(String, String)>,
}

impl BufferedHour {
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct PendingFoldReceipt {
    pub buffer_filenames: Vec<String>,
    pub previous_metadata: Option<String>,
}

pub(super) fn pending_fold_receipt(metadata: Option<&str>) -> Option<PendingFoldReceipt> {
    serde_json::from_str(metadata?).ok()
}

pub(super) fn clear_pending_fold_receipts(
    config: &MemoryConfig,
    namespace: &str,
    hour_ids: &[String],
) -> Result<()> {
    for hour_id in hour_ids {
        let Some(mut node) = store::read_node(config, namespace, hour_id)? else {
            continue;
        };
        let Some(receipt) = pending_fold_receipt(node.metadata.as_deref()) else {
            continue;
        };
        node.metadata = receipt.previous_metadata;
        store::write_node(config, &node)?;
    }
    Ok(())
}

/// Group buffer entries by hour from their filename timestamps.
pub(super) fn group_by_hour(entries: &[(String, String)]) -> BTreeMap<String, BufferedHour> {
    let mut groups: BTreeMap<String, BufferedHour> = BTreeMap::new();
    for (filename, content) in entries {
        let hour_id = hour_id_from_buffer_filename(filename).unwrap_or_else(|| {
            let (hour, _, _, _, _) = derive_node_ids(&Utc::now());
            hour
        });
        groups
            .entry(hour_id)
            .or_default()
            .entries
            .push((filename.clone(), content.clone()));
    }
    groups
}

/// Extract the hour node ID from a buffer filename like `1711972800000_abc.md`.
fn hour_id_from_buffer_filename(filename: &str) -> Option<String> {
    let ts_str = filename.split('_').next()?;
    let millis: i64 = ts_str.parse().ok()?;
    let dt = DateTime::from_timestamp_millis(millis)?;
    let (hour_id, _, _, _, _) = derive_node_ids(&dt);
    Some(hour_id)
}
