//! Crash-safe filesystem staging and swap helpers for time-tree rebuilds.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::store;
use super::types::{level_from_node_id, NodeLevel, TreeNode};
use crate::memory::config::MemoryConfig;

const STAGING_WORKSPACE: &str = ".memory-rebuild-staging";
const BACKUP_DIR: &str = "tree.rebuild-backup";

pub(super) struct StagedTree {
    pub config: MemoryConfig,
    active: PathBuf,
    staged: PathBuf,
    backup: PathBuf,
}

/// Restore an interrupted directory swap and adopt any buffer left in backup.
pub(super) fn recover_interrupted_swap(config: &MemoryConfig, namespace: &str) -> Result<()> {
    let active = store::tree_dir(config, namespace);
    let backup = active.with_file_name(BACKUP_DIR);
    if !backup.exists() {
        return Ok(());
    }
    if !active.exists() {
        std::fs::rename(&backup, &active).context("restore interrupted tree rebuild")?;
        return Ok(());
    }

    let backup_buffer = backup.join("buffer");
    if backup_buffer.exists() {
        let active_buffer = active.join("buffer");
        std::fs::create_dir_all(&active_buffer)?;
        for entry in std::fs::read_dir(&backup_buffer)? {
            let entry = entry?;
            let destination = active_buffer.join(entry.file_name());
            if !destination.exists() {
                std::fs::rename(entry.path(), destination)?;
            }
        }
    }
    std::fs::remove_dir_all(&backup).context("remove adopted tree rebuild backup")?;
    Ok(())
}

/// Create an empty sibling workspace where a replacement tree can be built.
pub(super) fn prepare(config: &MemoryConfig, namespace: &str) -> Result<StagedTree> {
    recover_interrupted_swap(config, namespace)?;
    let active = store::tree_dir(config, namespace);
    let backup = active.with_file_name(BACKUP_DIR);
    let mut staged_config = config.clone();
    staged_config.workspace = config.workspace.join(STAGING_WORKSPACE);
    let staged = store::tree_dir(&staged_config, namespace);
    if staged.exists() {
        std::fs::remove_dir_all(&staged).context("remove stale tree rebuild staging directory")?;
    }
    Ok(StagedTree {
        config: staged_config,
        active,
        staged,
        backup,
    })
}

/// Atomically publish a fully-built staged tree while preserving the latest
/// live ingestion buffer. Interrupted swaps are recoverable by
/// [`recover_interrupted_swap`].
pub(super) fn publish(staged: StagedTree) -> Result<()> {
    if let Some(parent) = staged.active.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if staged.backup.exists() {
        std::fs::remove_dir_all(&staged.backup)?;
    }
    if staged.active.exists() {
        std::fs::rename(&staged.active, &staged.backup)
            .context("move active tree to rebuild backup")?;
    }
    if let Err(error) = std::fs::rename(&staged.staged, &staged.active) {
        if !staged.active.exists() && staged.backup.exists() {
            let _ = std::fs::rename(&staged.backup, &staged.active);
        }
        return Err(error).context("publish rebuilt tree");
    }

    // The staging tree never contains a buffer. Move the latest live buffer
    // from the backup only after the new summary tree is visible.
    recover_interrupted_swap_for_paths(&staged.active, &staged.backup)
}

fn recover_interrupted_swap_for_paths(active: &Path, backup: &Path) -> Result<()> {
    if !backup.exists() {
        return Ok(());
    }
    let backup_buffer = backup.join("buffer");
    if backup_buffer.exists() {
        let active_buffer = active.join("buffer");
        std::fs::create_dir_all(&active_buffer)?;
        for entry in std::fs::read_dir(&backup_buffer)? {
            let entry = entry?;
            let destination = active_buffer.join(entry.file_name());
            if !destination.exists() {
                std::fs::rename(entry.path(), destination)?;
            }
        }
    }
    std::fs::remove_dir_all(backup).context("remove tree rebuild backup")?;
    Ok(())
}

/// Recursively collect all hour leaf nodes from a tree directory.
pub(super) fn collect_hour_leaves(
    dir: &Path,
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
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            if name == "buffer" || name == "buffer_backup" {
                continue;
            }
            let child_prefix = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            collect_hour_leaves(&entry.path(), namespace, &child_prefix, leaves)?;
        } else if file_type.is_file()
            && name.ends_with(".md")
            && name != "summary.md"
            && name != "root.md"
        {
            let hour = name.trim_end_matches(".md");
            let node_id = if prefix.is_empty() {
                hour.to_string()
            } else {
                format!("{prefix}/{hour}")
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

#[cfg(test)]
#[path = "rebuild_fs_tests.rs"]
mod tests;
