//! Ingestion-buffer operations for the markdown time-tree.

use std::path::PathBuf;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde_json::Value;

use super::paths::buffer_dir;
use crate::memory::config::MemoryConfig;

/// Append raw content to the ingestion buffer as a timestamped file.
pub fn buffer_write(
    config: &MemoryConfig,
    namespace: &str,
    content: &str,
    ts: &DateTime<Utc>,
    metadata: Option<&Value>,
) -> Result<PathBuf> {
    let dir = buffer_dir(config, namespace);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("create buffer dir {}", dir.display()))?;
    let filename = format!(
        "{}_{}.md",
        ts.timestamp_millis(),
        &uuid::Uuid::new_v4().to_string()[..8]
    );
    let path = dir.join(&filename);
    let file_content = if let Some(meta) = metadata {
        let meta_str = serde_json::to_string(meta).unwrap_or_default();
        format!("---\nmetadata: {meta_str}\n---\n\n{content}")
    } else {
        content.to_string()
    };
    std::fs::write(&path, file_content)
        .with_context(|| format!("write buffer entry {}", path.display()))?;
    Ok(path)
}

/// Read all buffered entries non-destructively, sorted by filename.
pub fn buffer_read(config: &MemoryConfig, namespace: &str) -> Result<Vec<(String, String)>> {
    let dir = buffer_dir(config, namespace);
    if !dir.exists() {
        return Ok(vec![]);
    }
    let mut entries: Vec<(String, PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().map(|e| e == "md").unwrap_or(false) {
            entries.push((entry.file_name().to_string_lossy().to_string(), path));
        }
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let mut contents = Vec::with_capacity(entries.len());
    for (name, path) in &entries {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("read buffer entry {}", path.display()))?;
        contents.push((name.clone(), strip_buffer_frontmatter(&raw)));
    }
    Ok(contents)
}

/// Delete specific buffer entries by filename.
pub fn buffer_delete(config: &MemoryConfig, namespace: &str, filenames: &[String]) -> Result<()> {
    let dir = buffer_dir(config, namespace);
    for name in filenames {
        let path = dir.join(name);
        if path.exists() {
            std::fs::remove_file(&path)
                .with_context(|| format!("failed to remove buffer entry '{name}'"))?;
        }
    }
    Ok(())
}

/// Read and drain all buffered entries.
pub fn buffer_drain(config: &MemoryConfig, namespace: &str) -> Result<Vec<(String, String)>> {
    let entries = buffer_read(config, namespace)?;
    if entries.is_empty() {
        return Ok(entries);
    }
    let filenames: Vec<String> = entries.iter().map(|(name, _)| name.clone()).collect();
    buffer_delete(config, namespace, &filenames)?;
    Ok(entries)
}

fn strip_buffer_frontmatter(raw: &str) -> String {
    let trimmed = raw.trim_start();
    if !trimmed.starts_with("---") {
        return raw.to_string();
    }
    let after_open = &trimmed[3..];
    if let Some(close_pos) = after_open.find("\n---") {
        after_open[close_pos + 4..]
            .trim_start_matches('\n')
            .to_string()
    } else {
        raw.to_string()
    }
}
