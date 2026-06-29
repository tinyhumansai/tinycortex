//! Tree scanning: node counts, status, deletion, and root-summary collection.

use std::path::Path;

use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};

use super::nodes::read_node;
use super::paths::tree_dir;
use crate::memory::config::MemoryConfig;
use crate::memory::tree::runtime::types::TreeStatus;

/// Recursively count all `.md` files in the tree directory.
pub fn count_nodes(config: &MemoryConfig, namespace: &str) -> Result<u64> {
    let base = tree_dir(config, namespace);
    if !base.exists() {
        return Ok(0);
    }
    count_md_files(&base)
}

/// Scan the tree to produce a status summary.
pub fn get_tree_status(config: &MemoryConfig, namespace: &str) -> Result<TreeStatus> {
    let base = tree_dir(config, namespace);
    let total_nodes = if base.exists() {
        count_md_files(&base)?
    } else {
        0
    };

    let mut depth = 0u32;
    if base.join("root.md").exists() {
        depth = 1;
    }
    let mut oldest: Option<DateTime<Utc>> = None;
    let mut newest: Option<DateTime<Utc>> = None;

    if base.exists() {
        for entry in std::fs::read_dir(&base).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) && name.len() == 4 {
                depth = depth.max(2);
                for month_entry in std::fs::read_dir(entry.path())
                    .into_iter()
                    .flatten()
                    .flatten()
                {
                    if month_entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                        depth = depth.max(3);
                        for day_entry in std::fs::read_dir(month_entry.path())
                            .into_iter()
                            .flatten()
                            .flatten()
                        {
                            if day_entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                                depth = depth.max(4);
                                for hour_entry in std::fs::read_dir(day_entry.path())
                                    .into_iter()
                                    .flatten()
                                    .flatten()
                                {
                                    let hname =
                                        hour_entry.file_name().to_string_lossy().to_string();
                                    if hname.ends_with(".md") && hname != "summary.md" {
                                        depth = depth.max(5);
                                        if let Some(ts) = timestamp_from_hour_path(
                                            &name,
                                            month_entry.file_name().to_string_lossy().as_ref(),
                                            day_entry.file_name().to_string_lossy().as_ref(),
                                            &hname,
                                        ) {
                                            oldest = Some(oldest.map_or(ts, |o| o.min(ts)));
                                            newest = Some(newest.map_or(ts, |n| n.max(ts)));
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(TreeStatus {
        namespace: namespace.to_string(),
        total_nodes,
        depth,
        oldest_entry: oldest,
        newest_entry: newest,
        last_run_at: None,
    })
}

/// Remove the entire tree directory for a namespace, returning the node count.
pub fn delete_tree(config: &MemoryConfig, namespace: &str) -> Result<u64> {
    let base = tree_dir(config, namespace);
    if !base.exists() {
        return Ok(0);
    }
    let count = count_md_files(&base)?;
    std::fs::remove_dir_all(&base).with_context(|| format!("delete tree at {}", base.display()))?;
    Ok(count)
}

/// Pull the root-level summary out of every namespace under `workspace_dir`,
/// capped per-namespace and in total. Best-effort: failures are dropped.
/// Returns a stable-ordered `Vec<(namespace, body, updated_at)>`.
pub fn collect_root_summaries_with_caps(
    workspace_dir: &Path,
    per_namespace_cap: usize,
    total_cap: usize,
) -> Vec<(String, String, DateTime<Utc>)> {
    let config = MemoryConfig::new(workspace_dir);
    let namespaces = match list_namespaces_with_root(&config) {
        Ok(list) => list,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    let mut total_chars: usize = 0;
    for ns in namespaces {
        if total_chars >= total_cap {
            break;
        }
        let node = match read_node(&config, &ns, "root") {
            Ok(Some(node)) => node,
            _ => continue,
        };
        let body = node.summary.trim();
        if body.is_empty() {
            continue;
        }
        let body_chars = body.chars().count();
        let truncated: String = if body_chars > per_namespace_cap {
            body.chars().take(per_namespace_cap).collect::<String>() + "\n\n[... truncated]"
        } else {
            body.to_string()
        };
        let truncated_chars = truncated.chars().count();
        let remaining = total_cap.saturating_sub(total_chars);
        let final_body = if truncated_chars > remaining {
            let mut clipped: String = truncated.chars().take(remaining).collect();
            clipped.push_str("\n\n[... truncated]");
            clipped
        } else {
            truncated
        };
        total_chars += final_body.chars().count();
        out.push((ns, final_body, node.updated_at));
    }
    out
}

/// Enumerate namespaces under the workspace that have a `root.md` summary,
/// in stable sorted order.
pub fn list_namespaces_with_root(config: &MemoryConfig) -> Result<Vec<String>> {
    let base = config.workspace.join("memory").join("namespaces");
    if !base.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&base)
        .with_context(|| format!("scan namespaces dir {}", base.display()))?
    {
        let entry = entry?;
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let ns_name = entry.file_name().to_string_lossy().to_string();
        if entry.path().join("tree").join("root.md").exists() {
            out.push(ns_name);
        }
    }
    out.sort();
    Ok(out)
}

fn count_md_files(dir: &Path) -> Result<u64> {
    let mut count = 0u64;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let ft = entry.file_type()?;
        if ft.is_dir() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name == "buffer" || name == "buffer_backup" {
                continue;
            }
            count += count_md_files(&entry.path())?;
        } else if ft.is_file() && entry.path().extension().map(|e| e == "md").unwrap_or(false) {
            count += 1;
        }
    }
    Ok(count)
}

fn timestamp_from_hour_path(
    year: &str,
    month: &str,
    day: &str,
    hour_file: &str,
) -> Option<DateTime<Utc>> {
    let hour = hour_file.trim_end_matches(".md");
    let y: i32 = year.parse().ok()?;
    let m: u32 = month.parse().ok()?;
    let d: u32 = day.parse().ok()?;
    let h: u32 = hour.parse().ok()?;
    Utc.with_ymd_and_hms(y, m, d, h, 0, 0).single()
}
