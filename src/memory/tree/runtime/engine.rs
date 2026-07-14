//! Core summarisation engine for the markdown time-tree: drain the ingestion
//! buffer, summarise into hour leaves, and propagate summaries upward through
//! day → month → year → root.
//!
//! Ported from OpenHuman's `tree_runtime/engine.rs`. The LLM `Provider` is
//! abstracted behind the [`Summariser`] trait (with no model id threaded
//! through); event-bus progress events and the background hourly loop are not
//! ported.

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};

use super::fold::{
    clear_pending_fold_receipts, group_by_hour, pending_fold_receipt, PendingFoldReceipt,
};
use super::store;
use super::types::{derive_parent_id, estimate_tokens, NodeLevel, TreeNode, TreeStatus};
use crate::memory::config::MemoryConfig;

/// Maximum characters for a summary response (hard cap after the summariser call).
const MAX_SUMMARY_CHARS: usize = 20_000 * 4;

/// Single-text summariser backing the time-tree engine. Abstracts the LLM so
/// the crate never makes a network call; tests supply a deterministic stub.
#[async_trait]
pub trait Summariser: Send + Sync {
    /// Summarise `content` under an optional system instruction.
    async fn summarise(&self, system: Option<&str>, content: &str) -> Result<String>;
}

/// Product hook for runtime progress notifications. Implementations must not
/// retain PII-bearing summary content; only structural identifiers and counts
/// are exposed here.
pub trait RuntimeObserver: Send + Sync {
    fn hour_completed(&self, _namespace: &str, _node_id: &str, _token_count: u32) {}
    fn node_propagated(
        &self,
        _namespace: &str,
        _node_id: &str,
        _level: NodeLevel,
        _token_count: u32,
    ) {
    }
    fn rebuild_completed(&self, _namespace: &str, _total_nodes: u64) {}
}

struct NoopObserver;
impl RuntimeObserver for NoopObserver {}

/// Run the summarisation job for a namespace: drain the buffer, group entries
/// by hour, summarise each hour into its leaf, then propagate upward. Returns
/// the last hour leaf created, or `None` if the buffer was empty.
///
/// The buffer is only deleted (`store::buffer_delete`) when every propagation
/// step succeeds, so a transient failure leaves the raw entries in place for
/// the next run to retry.
///
/// Hour leaves carry an internal receipt for the buffer filenames already
/// folded into them. If upper-level propagation fails, a retry skips those
/// entries while still incorporating entries appended after the failed run.
///
/// `_ts` is currently unused — hour bucketing is derived from each buffer
/// entry's own filename timestamp, not from this parameter.
pub async fn run_summarization(
    config: &MemoryConfig,
    summariser: &dyn Summariser,
    namespace: &str,
    _ts: DateTime<Utc>,
) -> Result<Option<TreeNode>> {
    run_summarization_observed(config, summariser, namespace, _ts, &NoopObserver).await
}

pub async fn run_summarization_observed(
    config: &MemoryConfig,
    summariser: &dyn Summariser,
    namespace: &str,
    _ts: DateTime<Utc>,
    observer: &dyn RuntimeObserver,
) -> Result<Option<TreeNode>> {
    super::rebuild_fs::recover_interrupted_swap(config, namespace)?;
    let buffered = store::buffer_read(config, namespace)?;
    if buffered.is_empty() {
        return Ok(None);
    }
    let buffer_filenames: Vec<String> = buffered.iter().map(|(name, _)| name.clone()).collect();
    let hour_groups = group_by_hour(&buffered);

    let mut all_propagation_ids: Vec<(String, NodeLevel)> = Vec::new();
    let mut last_hour_node: Option<TreeNode> = None;
    let mut pending_hour_ids = Vec::new();

    for (hour_id, group) in &hour_groups {
        let existing = store::read_node(config, namespace, hour_id)?;
        let receipt = existing
            .as_ref()
            .and_then(|node| pending_fold_receipt(node.metadata.as_deref()));
        let already_applied: std::collections::HashSet<&str> = receipt
            .as_ref()
            .map(|receipt| {
                receipt
                    .buffer_filenames
                    .iter()
                    .map(String::as_str)
                    .collect()
            })
            .unwrap_or_default();
        let new_entries = group
            .entries
            .iter()
            .filter(|(filename, _)| !already_applied.contains(filename.as_str()))
            .collect::<Vec<_>>();
        let new_content = new_entries
            .iter()
            .map(|(_, content)| content.as_str())
            .collect::<Vec<_>>()
            .join("\n\n---\n\n");

        let hour_node = if new_entries.is_empty() {
            existing.context("pending hour receipt exists without an hour node")?
        } else {
            let to_summarize = match existing.as_ref() {
                Some(previous) => format!("{}\n\n---\n\n{new_content}", previous.summary),
                None => new_content,
            };
            let hour_summary = summarize_to_limit(
                summariser,
                &to_summarize,
                NodeLevel::Hour.max_tokens(),
                "hour",
                hour_id,
            )
            .await
            .context("summarize hour leaf")?;
            let now = Utc::now();
            let previous_metadata = receipt
                .as_ref()
                .and_then(|receipt| receipt.previous_metadata.clone())
                .or_else(|| existing.as_ref().and_then(|node| node.metadata.clone()));
            let metadata = serde_json::to_string(&PendingFoldReceipt {
                buffer_filenames: group
                    .entries
                    .iter()
                    .map(|(filename, _)| filename.clone())
                    .collect(),
                previous_metadata,
            })?;
            let node = TreeNode {
                node_id: hour_id.clone(),
                namespace: namespace.to_string(),
                level: NodeLevel::Hour,
                parent_id: derive_parent_id(hour_id),
                summary: hour_summary.clone(),
                token_count: estimate_tokens(&hour_summary),
                child_count: 0,
                created_at: existing.as_ref().map(|node| node.created_at).unwrap_or(now),
                updated_at: now,
                metadata: Some(metadata),
            };
            store::write_node(config, &node)?;
            node
        };
        observer.hour_completed(namespace, hour_id, hour_node.token_count);

        let (_, day_id, month_id, year_id, root_id) = derive_node_ids_from_hour_id(hour_id);
        all_propagation_ids.push((day_id, NodeLevel::Day));
        all_propagation_ids.push((month_id, NodeLevel::Month));
        all_propagation_ids.push((year_id, NodeLevel::Year));
        all_propagation_ids.push((root_id, NodeLevel::Root));
        last_hour_node = Some(hour_node);
        pending_hour_ids.push(hour_id.clone());
    }

    // Propagate bottom-up; a single node's failure does not void the whole run.
    let mut seen = std::collections::HashSet::new();
    let mut failed: Vec<String> = Vec::new();
    for level in [
        NodeLevel::Day,
        NodeLevel::Month,
        NodeLevel::Year,
        NodeLevel::Root,
    ] {
        for (node_id, node_level) in &all_propagation_ids {
            if *node_level == level && seen.insert(node_id.clone()) {
                if let Err(_e) =
                    propagate_node_observed(config, summariser, namespace, node_id, level, observer)
                        .await
                {
                    failed.push(node_id.clone());
                }
            }
        }
    }

    // Clear the buffer only on full success so transient failures retry.
    if failed.is_empty() {
        store::buffer_delete(config, namespace, &buffer_filenames)
            .context("delete buffer entries after successful summarization")?;
        clear_pending_fold_receipts(config, namespace, &pending_hour_ids)?;
    }
    Ok(last_hour_node)
}

/// Rebuild the entire tree from hour leaves upward, preserving unsummarised
/// buffer content. The replacement is built in a staging workspace and then
/// published with a recoverable directory swap, so a process crash cannot
/// expose a partially rebuilt tree or strand the live ingestion buffer.
///
/// # Errors
/// Propagates any filesystem error from the delete/rename/rewrite steps.
/// Individual `propagate_node` failures during the day/month/year/root
/// re-summarisation pass are swallowed (best-effort) so one bad node doesn't
/// abort the whole rebuild.
pub async fn rebuild_tree(
    config: &MemoryConfig,
    summariser: &dyn Summariser,
    namespace: &str,
) -> Result<TreeStatus> {
    rebuild_tree_observed(config, summariser, namespace, &NoopObserver).await
}

pub async fn rebuild_tree_observed(
    config: &MemoryConfig,
    summariser: &dyn Summariser,
    namespace: &str,
    observer: &dyn RuntimeObserver,
) -> Result<TreeStatus> {
    super::rebuild_fs::recover_interrupted_swap(config, namespace)?;
    let status = store::get_tree_status(config, namespace)?;
    if status.total_nodes == 0 {
        return Ok(status);
    }

    let base = store::tree_dir(config, namespace);
    let mut hour_leaves: Vec<TreeNode> = Vec::new();
    super::rebuild_fs::collect_hour_leaves(&base, namespace, "", &mut hour_leaves)?;
    if hour_leaves.is_empty() {
        return store::get_tree_status(config, namespace);
    }

    let staged = super::rebuild_fs::prepare(config, namespace)?;

    for leaf in &hour_leaves {
        store::write_node(&staged.config, leaf)?;
    }

    let mut day_ids = std::collections::BTreeSet::new();
    let mut month_ids = std::collections::BTreeSet::new();
    let mut year_ids = std::collections::BTreeSet::new();
    for leaf in &hour_leaves {
        if let Some(day) = derive_parent_id(&leaf.node_id) {
            if let Some(month) = derive_parent_id(&day) {
                if let Some(year) = derive_parent_id(&month) {
                    year_ids.insert(year);
                }
                month_ids.insert(month);
            }
            day_ids.insert(day);
        }
    }

    // Partial-success propagation: a single node's failure does not abort the
    // rebuild (the hour leaves are already re-written above).
    for day_id in &day_ids {
        let _ = propagate_node_observed(
            &staged.config,
            summariser,
            namespace,
            day_id,
            NodeLevel::Day,
            observer,
        )
        .await;
    }
    for month_id in &month_ids {
        let _ = propagate_node_observed(
            &staged.config,
            summariser,
            namespace,
            month_id,
            NodeLevel::Month,
            observer,
        )
        .await;
    }
    for year_id in &year_ids {
        let _ = propagate_node_observed(
            &staged.config,
            summariser,
            namespace,
            year_id,
            NodeLevel::Year,
            observer,
        )
        .await;
    }
    let _ = propagate_node_observed(
        &staged.config,
        summariser,
        namespace,
        "root",
        NodeLevel::Root,
        observer,
    )
    .await;

    super::rebuild_fs::publish(staged)?;
    let status = store::get_tree_status(config, namespace)?;
    observer.rebuild_completed(namespace, status.total_nodes);
    Ok(status)
}

/// Re-summarise a single non-leaf node from its children.
#[cfg(test)]
pub(crate) async fn propagate_node(
    config: &MemoryConfig,
    summariser: &dyn Summariser,
    namespace: &str,
    node_id: &str,
    level: NodeLevel,
) -> Result<()> {
    propagate_node_observed(config, summariser, namespace, node_id, level, &NoopObserver).await
}

async fn propagate_node_observed(
    config: &MemoryConfig,
    summariser: &dyn Summariser,
    namespace: &str,
    node_id: &str,
    level: NodeLevel,
    observer: &dyn RuntimeObserver,
) -> Result<()> {
    let children = store::read_children(config, namespace, node_id)?;
    if children.is_empty() {
        return Ok(());
    }
    let child_count = children.len() as u32;
    let combined: String = children
        .iter()
        .map(|c| format!("## {} ({})\n\n{}", c.node_id, c.level.as_str(), c.summary))
        .collect::<Vec<_>>()
        .join("\n\n---\n\n");
    let combined_tokens = estimate_tokens(&combined);
    let max_tokens = level.max_tokens();

    let summary = if combined_tokens <= max_tokens {
        combined
    } else {
        summarize_to_limit(summariser, &combined, max_tokens, level.as_str(), node_id).await?
    };

    let now = Utc::now();
    let created_at = store::read_node(config, namespace, node_id)?
        .map(|n| n.created_at)
        .unwrap_or(now);
    let node = TreeNode {
        node_id: node_id.to_string(),
        namespace: namespace.to_string(),
        level,
        parent_id: derive_parent_id(node_id),
        summary: summary.clone(),
        token_count: estimate_tokens(&summary),
        child_count,
        created_at,
        updated_at: now,
        metadata: None,
    };
    store::write_node(config, &node)?;
    observer.node_propagated(namespace, node_id, level, node.token_count);
    Ok(())
}

/// Summarise text to fit within a token limit, enforcing a hard char cap on the
/// summariser's response.
async fn summarize_to_limit(
    summariser: &dyn Summariser,
    content: &str,
    max_tokens: u32,
    level_name: &str,
    node_id: &str,
) -> Result<String> {
    let max_chars = (max_tokens as usize) * 4;
    let system_prompt = format!(
        "You are a hierarchical summarizer. Compress the following content into a concise \
         summary that preserves the most important information.\n\n\
         Rules:\n\
         - The summary MUST be under {max_tokens} tokens (roughly {max_chars} characters).\n\
         - Focus on key events, decisions, facts, patterns, and actionable insights.\n\
         - Preserve names, dates, numbers, and specific details when important.\n\
         - Use clear, dense prose — no filler.\n\n\
         Context: You are summarizing at the {level_name} level for node '{node_id}'.",
    );
    let response = summariser
        .summarise(Some(&system_prompt), content)
        .await
        .with_context(|| format!("summarization failed for node {node_id} (level={level_name})"))?;

    let char_limit = max_chars.min(MAX_SUMMARY_CHARS);
    let response = if response.len() > char_limit {
        response[..floor_char_boundary(&response, char_limit)].to_string()
    } else {
        response
    };
    Ok(response)
}

/// Derive propagation IDs from an hour node_id like "2024/03/15/14".
fn derive_node_ids_from_hour_id(hour_id: &str) -> (String, String, String, String, String) {
    let parts: Vec<&str> = hour_id.split('/').collect();
    if parts.len() == 4 {
        let year = parts[0].to_string();
        let month = format!("{}/{}", parts[0], parts[1]);
        let day = format!("{}/{}/{}", parts[0], parts[1], parts[2]);
        (hour_id.to_string(), day, month, year, "root".to_string())
    } else {
        (
            hour_id.to_string(),
            "unknown".to_string(),
            "unknown".to_string(),
            "unknown".to_string(),
            "root".to_string(),
        )
    }
}

/// Discover namespaces that have pending buffer data.
pub fn discover_active_namespaces(config: &MemoryConfig) -> Vec<String> {
    let namespaces_dir = config.workspace.join("memory").join("namespaces");
    if !namespaces_dir.exists() {
        return vec![];
    }
    let mut active = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&namespaces_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let buffer_dir = entry.path().join("tree").join("buffer");
            if buffer_dir.exists() {
                if let Ok(buffer_entries) = std::fs::read_dir(&buffer_dir) {
                    let has = buffer_entries
                        .flatten()
                        .any(|e| e.path().extension().map(|ext| ext == "md").unwrap_or(false));
                    if has {
                        active.push(name);
                    }
                }
            }
        }
    }
    active
}

/// Largest index `<= index` that lies on a UTF-8 char boundary.
fn floor_char_boundary(s: &str, index: usize) -> usize {
    if index >= s.len() {
        return s.len();
    }
    let mut i = index;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

#[cfg(test)]
#[path = "engine_tests.rs"]
mod tests;
