//! Node read/write + markdown (de)serialisation for the time-tree.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};

use super::paths::{node_file_path, tree_dir};
use crate::memory::config::MemoryConfig;
use crate::memory::tree::runtime::types::{
    derive_parent_id, estimate_tokens, level_from_node_id, NodeLevel, TreeNode,
};

/// Write a tree node to disk as a markdown file with YAML frontmatter.
///
/// The replacement uses the crate's same-directory temp-file + rename helper,
/// so readers observe either the previous complete node or the new one, never
/// a partially-written markdown file (TR-7).
pub fn write_node(config: &MemoryConfig, node: &TreeNode) -> Result<()> {
    let path = node_file_path(config, &node.namespace, &node.node_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create dirs for {}", parent.display()))?;
    }
    let metadata_line = match &node.metadata {
        Some(m) => format!("metadata: {}\n", yaml_string(m)),
        None => String::new(),
    };
    let frontmatter = format!(
        "---\n\
         node_id: {}\n\
         namespace: {}\n\
         level: {}\n\
         parent_id: {}\n\
         token_count: {}\n\
         child_count: {}\n\
         created_at: {}\n\
         updated_at: {}\n\
         {}\
         ---\n\n",
        yaml_string(&node.node_id),
        yaml_string(&node.namespace),
        node.level.as_str(),
        match &node.parent_id {
            Some(pid) => yaml_string(pid),
            None => "~".to_string(),
        },
        node.token_count,
        node.child_count,
        node.created_at.to_rfc3339(),
        node.updated_at.to_rfc3339(),
        metadata_line,
    );
    let content = format!("{frontmatter}{}\n", node.summary);
    // Atomic write: a crash mid-write must not leave a tree node file truncated
    // or empty, which would corrupt the summary hierarchy on the next read.
    crate::memory::fsutil::atomic_write(&path, content.as_bytes())
        .with_context(|| format!("write tree node {}", path.display()))?;
    Ok(())
}

fn yaml_string(value: &str) -> String {
    serde_json::to_string(value).expect("serializing a string cannot fail")
}

/// Read a single tree node from its markdown file. `None` if it does not exist.
pub fn read_node(
    config: &MemoryConfig,
    namespace: &str,
    node_id: &str,
) -> Result<Option<TreeNode>> {
    let path = node_file_path(config, namespace, node_id);
    if !path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("read tree node {}", path.display()))?;
    parse_node_markdown(&raw, namespace, node_id).map(Some)
}

/// Read all direct children of a node.
pub fn read_children(
    config: &MemoryConfig,
    namespace: &str,
    parent_id: &str,
) -> Result<Vec<TreeNode>> {
    let parent_level = level_from_node_id(parent_id);
    let base = tree_dir(config, namespace);
    match parent_level {
        NodeLevel::Root => read_subdirectory_summaries(&base, namespace, ""),
        NodeLevel::Year | NodeLevel::Month => {
            read_subdirectory_summaries(&base, namespace, parent_id)
        }
        NodeLevel::Day => read_hour_leaves(&base, namespace, parent_id),
        NodeLevel::Hour => Ok(vec![]),
    }
}

/// Walk up from a node to the root, returning all ancestors.
pub fn read_ancestors(
    config: &MemoryConfig,
    namespace: &str,
    node_id: &str,
) -> Result<Vec<TreeNode>> {
    let mut ancestors = Vec::new();
    let mut current = derive_parent_id(node_id);
    while let Some(pid) = current {
        if let Some(node) = read_node(config, namespace, &pid)? {
            ancestors.push(node);
        }
        current = derive_parent_id(&pid);
    }
    Ok(ancestors)
}

/// List a level's `summary.md` children (year/month directories) under
/// `base/parent_id` (or `base` itself when `parent_id` is empty, i.e. the
/// root's year children). Skips `buffer`/`buffer_backup` and any non-numeric
/// directory name; a directory without a `summary.md` is silently omitted.
/// Sorted by `node_id` ascending. Read errors on individual entries are
/// swallowed (`if let Ok(...)`), not propagated.
fn read_subdirectory_summaries(
    base: &Path,
    namespace: &str,
    parent_id: &str,
) -> Result<Vec<TreeNode>> {
    let scan_dir = if parent_id.is_empty() {
        base.to_path_buf()
    } else {
        base.join(parent_id)
    };
    if !scan_dir.exists() {
        return Ok(vec![]);
    }
    let mut children = Vec::new();
    for entry in std::fs::read_dir(&scan_dir)? {
        let entry = entry?;
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let child_name = entry.file_name().to_string_lossy().to_string();
        if child_name == "buffer"
            || child_name == "buffer_backup"
            || child_name.chars().any(|c| !c.is_ascii_digit())
        {
            continue;
        }
        let child_id = if parent_id.is_empty() {
            child_name
        } else {
            format!("{parent_id}/{child_name}")
        };
        let summary_path = entry.path().join("summary.md");
        if summary_path.exists() {
            let raw = std::fs::read_to_string(&summary_path)?;
            if let Ok(node) = parse_node_markdown(&raw, namespace, &child_id) {
                children.push(node);
            }
        }
    }
    children.sort_by(|a, b| a.node_id.cmp(&b.node_id));
    Ok(children)
}

/// List the hour-leaf `.md` files directly under `base/day_id`, excluding
/// `summary.md`. Sorted by `node_id` ascending; a file that fails to parse is
/// silently skipped rather than propagating the error.
fn read_hour_leaves(base: &Path, namespace: &str, day_id: &str) -> Result<Vec<TreeNode>> {
    let day_dir = base.join(day_id);
    if !day_dir.exists() {
        return Ok(vec![]);
    }
    let mut leaves = Vec::new();
    for entry in std::fs::read_dir(&day_dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.ends_with(".md") || name == "summary.md" {
            continue;
        }
        let node_id = format!("{day_id}/{}", name.trim_end_matches(".md"));
        let raw = std::fs::read_to_string(entry.path())?;
        if let Ok(node) = parse_node_markdown(&raw, namespace, &node_id) {
            leaves.push(node);
        }
    }
    leaves.sort_by(|a, b| a.node_id.cmp(&b.node_id));
    Ok(leaves)
}

/// Public entry point for parsing a markdown node (used by engine rebuild).
pub fn parse_node_markdown_pub(raw: &str, namespace: &str, node_id: &str) -> Result<TreeNode> {
    parse_node_markdown(raw, namespace, node_id)
}

/// Parse a node markdown file's YAML-ish frontmatter + body into a [`TreeNode`].
/// Every field falls back to a derived or default value when missing or
/// unparsable (e.g. `level` falls back to [`level_from_node_id`], timestamps
/// fall back to [`DateTime::<Utc>::UNIX_EPOCH`]) — this function does not fail
/// on malformed frontmatter, which is what lets a truncated write (see the
/// `TR-7` note on [`write_node`]) go undetected.
fn parse_node_markdown(raw: &str, namespace: &str, node_id: &str) -> Result<TreeNode> {
    let (frontmatter, body_raw) = split_frontmatter(raw);
    let body = body_raw.trim_end().to_string();

    let level = frontmatter
        .get("level")
        .and_then(|v| NodeLevel::from_str_label(v))
        .unwrap_or_else(|| level_from_node_id(node_id));
    let parent_id = frontmatter
        .get("parent_id")
        .and_then(|v| {
            let trimmed = v.trim().trim_matches('"');
            if trimmed == "~" || trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        })
        .or_else(|| derive_parent_id(node_id));
    let token_count = frontmatter
        .get("token_count")
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or_else(|| estimate_tokens(&body));
    let child_count = frontmatter
        .get("child_count")
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(0);
    let created_at = frontmatter
        .get("created_at")
        .and_then(|v| DateTime::parse_from_rfc3339(v).ok())
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or(DateTime::<Utc>::UNIX_EPOCH);
    let updated_at = frontmatter
        .get("updated_at")
        .and_then(|v| DateTime::parse_from_rfc3339(v).ok())
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or(created_at);
    let metadata = frontmatter.get("metadata").map(|v| v.to_string());

    Ok(TreeNode {
        node_id: node_id.to_string(),
        namespace: namespace.to_string(),
        level,
        parent_id,
        summary: body,
        token_count,
        child_count,
        created_at,
        updated_at,
        metadata,
    })
}

/// Split markdown into a (frontmatter key-value map, body text) pair.
///
/// Looks for a leading `---` fence and the first subsequent `\n---`; each
/// `key: value` line in between is parsed with a single `find(':')` split
/// (values are trimmed and unwrapped of one layer of surrounding `"`). If the
/// content doesn't start with `---`, or no closing fence is found, the whole
/// input is returned unmodified as the body with an empty map — this function
/// never errors, it degrades to "no frontmatter" instead.
pub(crate) fn split_frontmatter(raw: &str) -> (HashMap<String, String>, String) {
    let mut map = HashMap::new();
    let trimmed = raw.trim_start();
    if !trimmed.starts_with("---") {
        return (map, raw.to_string());
    }
    let after_open = &trimmed[3..];
    if let Some(close_pos) = after_open.find("\n---") {
        let fm_block = &after_open[..close_pos];
        let body = after_open[close_pos + 4..]
            .trim_start_matches('\n')
            .to_string();
        for line in fm_block.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Some(colon_pos) = line.find(':') {
                let key = line[..colon_pos].trim().to_string();
                let raw_value = line[colon_pos + 1..].trim();
                let value = serde_json::from_str::<String>(raw_value)
                    .unwrap_or_else(|_| raw_value.trim_matches('"').to_string());
                map.insert(key, value);
            }
        }
        (map, body)
    } else {
        (map, raw.to_string())
    }
}
