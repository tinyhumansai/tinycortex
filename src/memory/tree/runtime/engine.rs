//! Core summarisation engine for the markdown time-tree: drain the ingestion
//! buffer, summarise into hour leaves, and propagate summaries upward through
//! day → month → year → root.
//!
//! Ported from OpenHuman's `tree_runtime/engine.rs`. The LLM `Provider` is
//! abstracted behind the [`Summariser`] trait (with no model id threaded
//! through); event-bus progress events and the background hourly loop are not
//! ported.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};

use super::store;
use super::types::{
    derive_node_ids, derive_parent_id, estimate_tokens, level_from_node_id, NodeLevel, TreeNode,
    TreeStatus,
};
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

/// Run the summarisation job for a namespace: drain the buffer, group entries
/// by hour, summarise each hour into its leaf, then propagate upward. Returns
/// the last hour leaf created, or `None` if the buffer was empty.
///
/// The buffer is only deleted (`store::buffer_delete`) when every propagation
/// step succeeds, so a transient failure leaves the raw entries in place for
/// the next run to retry.
///
/// # NOTE: retry after a partial failure can double-fold entries (`TR-11`)
/// Hour leaves are always written unconditionally before propagation runs; if
/// only an upper-level propagation fails (`_e` below is discarded, so the
/// specific failure is not surfaced to the caller), the next call re-reads the
/// buffer (not yet deleted), re-appends its content onto the *already updated*
/// hour leaf (`to_summarize` prepends `existing_summary`), and folds it a
/// second time. See `docs/spec/audit/03-tree-archivist-conversations.md`.
///
/// `_ts` is currently unused — hour bucketing is derived from each buffer
/// entry's own filename timestamp, not from this parameter.
pub async fn run_summarization(
    config: &MemoryConfig,
    summariser: &dyn Summariser,
    namespace: &str,
    _ts: DateTime<Utc>,
) -> Result<Option<TreeNode>> {
    let buffered = store::buffer_read(config, namespace)?;
    if buffered.is_empty() {
        return Ok(None);
    }
    let buffer_filenames: Vec<String> = buffered.iter().map(|(name, _)| name.clone()).collect();
    let hour_groups = group_by_hour(&buffered);

    let mut all_propagation_ids: Vec<(String, NodeLevel)> = Vec::new();
    let mut last_hour_node: Option<TreeNode> = None;

    for (hour_id, entries) in &hour_groups {
        let combined = entries.join("\n\n---\n\n");
        let (existing_summary, existing_created_at) =
            match store::read_node(config, namespace, hour_id)? {
                Some(existing) => (Some(existing.summary), Some(existing.created_at)),
                None => (None, None),
            };
        let to_summarize = match existing_summary {
            Some(prev) => format!("{prev}\n\n---\n\n{combined}"),
            None => combined,
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
        let hour_node = TreeNode {
            node_id: hour_id.clone(),
            namespace: namespace.to_string(),
            level: NodeLevel::Hour,
            parent_id: derive_parent_id(hour_id),
            summary: hour_summary.clone(),
            token_count: estimate_tokens(&hour_summary),
            child_count: 0,
            created_at: existing_created_at.unwrap_or(now),
            updated_at: now,
            metadata: None,
        };
        store::write_node(config, &hour_node)?;

        let (_, day_id, month_id, year_id, root_id) = derive_node_ids_from_hour_id(hour_id);
        all_propagation_ids.push((day_id, NodeLevel::Day));
        all_propagation_ids.push((month_id, NodeLevel::Month));
        all_propagation_ids.push((year_id, NodeLevel::Year));
        all_propagation_ids.push((root_id, NodeLevel::Root));
        last_hour_node = Some(hour_node);
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
                if let Err(_e) = propagate_node(config, summariser, namespace, node_id, level).await
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
    }
    Ok(last_hour_node)
}

/// Rebuild the entire tree from hour leaves upward, preserving unsummarised
/// buffer content.
///
/// # NOTE: not crash-safe (`TR-2`)
/// This deletes the whole on-disk tree directory ([`store::delete_tree`]) and
/// then rewrites every hour leaf from the in-memory `hour_leaves` vec. A crash
/// between the delete and the full rewrite permanently loses every summary in
/// the namespace — there is no atomic swap (rebuild-into-temp + rename). The
/// buffer-preservation dance around it has the same gap: if the process
/// crashes after the rename to `tree_buffer_backup` but before the restore
/// rename back, the backup directory is left orphaned — no code path on a
/// later run adopts it, so buffered (unsummarised) content is stranded outside
/// the active buffer dir. See `docs/spec/audit/03-tree-archivist-conversations.md`.
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
    let status = store::get_tree_status(config, namespace)?;
    if status.total_nodes == 0 {
        return Ok(status);
    }

    let base = store::tree_dir(config, namespace);
    let mut hour_leaves: Vec<TreeNode> = Vec::new();
    collect_hour_leaves_recursive(&base, namespace, "", &mut hour_leaves)?;
    if hour_leaves.is_empty() {
        return store::get_tree_status(config, namespace);
    }

    // Preserve the buffer outside the tree dir while we delete + rebuild.
    let buffer_path = store::buffer_dir(config, namespace);
    let tree_base = store::tree_dir(config, namespace);
    let buffer_backup = tree_base
        .parent()
        .unwrap_or(&tree_base)
        .join("tree_buffer_backup");
    let buffer_existed = buffer_path.exists();
    if buffer_existed {
        if buffer_backup.exists() {
            std::fs::remove_dir_all(&buffer_backup)?;
        }
        std::fs::rename(&buffer_path, &buffer_backup).context("backup buffer before rebuild")?;
    }
    store::delete_tree(config, namespace)?;
    if buffer_existed && buffer_backup.exists() {
        let restored = store::buffer_dir(config, namespace);
        if let Some(parent) = restored.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::rename(&buffer_backup, &restored).context("restore buffer after rebuild")?;
    }

    for leaf in &hour_leaves {
        store::write_node(config, leaf)?;
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
        let _ = propagate_node(config, summariser, namespace, day_id, NodeLevel::Day).await;
    }
    for month_id in &month_ids {
        let _ = propagate_node(config, summariser, namespace, month_id, NodeLevel::Month).await;
    }
    for year_id in &year_ids {
        let _ = propagate_node(config, summariser, namespace, year_id, NodeLevel::Year).await;
    }
    let _ = propagate_node(config, summariser, namespace, "root", NodeLevel::Root).await;

    store::get_tree_status(config, namespace)
}

/// Re-summarise a single non-leaf node from its children.
pub(crate) async fn propagate_node(
    config: &MemoryConfig,
    summariser: &dyn Summariser,
    namespace: &str,
    node_id: &str,
    level: NodeLevel,
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

/// Group buffer entries by hour from their filename timestamps.
pub(crate) fn group_by_hour(entries: &[(String, String)]) -> BTreeMap<String, Vec<String>> {
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (filename, content) in entries {
        let hour_id = hour_id_from_buffer_filename(filename).unwrap_or_else(|| {
            let (hour, _, _, _, _) = derive_node_ids(&Utc::now());
            hour
        });
        groups.entry(hour_id).or_default().push(content.clone());
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

/// Recursively collect all hour leaf nodes from the tree directory.
fn collect_hour_leaves_recursive(
    dir: &std::path::Path,
    namespace: &str,
    prefix: &str,
    leaves: &mut Vec<TreeNode>,
) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        let ft = entry.file_type()?;
        if ft.is_dir() {
            if name == "buffer" || name == "buffer_backup" {
                continue;
            }
            let child_prefix = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            collect_hour_leaves_recursive(&entry.path(), namespace, &child_prefix, leaves)?;
        } else if ft.is_file() && name.ends_with(".md") && name != "summary.md" && name != "root.md"
        {
            let hour_part = name.trim_end_matches(".md");
            let node_id = if prefix.is_empty() {
                hour_part.to_string()
            } else {
                format!("{prefix}/{hour_part}")
            };
            if level_from_node_id(&node_id) == NodeLevel::Hour {
                let raw = std::fs::read_to_string(entry.path())?;
                let node = store::parse_node_markdown_pub(&raw, namespace, &node_id)
                    .with_context(|| format!("failed to parse hour leaf '{node_id}'"))?;
                leaves.push(node);
            }
        }
    }
    Ok(())
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
