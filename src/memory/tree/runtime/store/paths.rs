//! Path helpers and id/namespace validation for the markdown time-tree.

use std::path::PathBuf;

use crate::memory::config::MemoryConfig;
use crate::memory::tree::runtime::types::node_id_to_path;

/// Base tree directory for a namespace.
pub fn tree_dir(config: &MemoryConfig, namespace: &str) -> PathBuf {
    config
        .workspace
        .join("memory")
        .join("namespaces")
        .join(sanitize(namespace))
        .join("tree")
}

/// Buffer directory where raw ingested content is staged before summarisation.
pub fn buffer_dir(config: &MemoryConfig, namespace: &str) -> PathBuf {
    tree_dir(config, namespace).join("buffer")
}

/// Absolute file path for a given node.
pub fn node_file_path(config: &MemoryConfig, namespace: &str, node_id: &str) -> PathBuf {
    tree_dir(config, namespace).join(node_id_to_path(node_id))
}

/// Sanitise a namespace string for use as a directory name.
fn sanitize(namespace: &str) -> String {
    namespace
        .trim()
        .replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|', '.'], "_")
        .replace("__", "_")
}

/// Validate a namespace string, erroring on empty / dangerous input.
pub fn validate_namespace(namespace: &str) -> Result<(), String> {
    let trimmed = namespace.trim();
    if trimmed.is_empty() {
        return Err("namespace must not be empty".to_string());
    }
    if trimmed.contains("..") {
        return Err("namespace must not contain '..'".to_string());
    }
    if trimmed.starts_with('/') || trimmed.starts_with('\\') {
        return Err("namespace must not start with a path separator".to_string());
    }
    Ok(())
}

/// Validate a node_id against the allowed canonical formats.
pub fn validate_node_id(node_id: &str) -> Result<(), String> {
    if node_id == "root" {
        return Ok(());
    }
    if node_id.contains("..") || node_id.starts_with('/') || node_id.ends_with('/') {
        return Err(format!(
            "invalid node_id '{node_id}': contains path traversal or leading/trailing slashes"
        ));
    }
    let parts: Vec<&str> = node_id.split('/').collect();
    if parts.is_empty() || parts.len() > 4 {
        return Err(format!(
            "invalid node_id '{node_id}': expected 1-4 segments (YYYY[/MM[/DD[/HH]]])"
        ));
    }
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            return Err(format!(
                "invalid node_id '{node_id}': empty segment at position {i}"
            ));
        }
        if !part.chars().all(|c| c.is_ascii_digit()) {
            return Err(format!(
                "invalid node_id '{node_id}': non-numeric segment '{part}' at position {i}"
            ));
        }
    }
    if parts.len() >= 2 {
        let month: u32 = parts[1].parse().unwrap_or(0);
        if !(1..=12).contains(&month) {
            return Err(format!(
                "invalid node_id '{node_id}': month {month} out of range 1-12"
            ));
        }
    }
    if parts.len() >= 3 {
        let day: u32 = parts[2].parse().unwrap_or(0);
        if !(1..=31).contains(&day) {
            return Err(format!(
                "invalid node_id '{node_id}': day {day} out of range 1-31"
            ));
        }
    }
    if parts.len() >= 4 {
        let hour: u32 = parts[3].parse().unwrap_or(99);
        if hour > 23 {
            return Err(format!(
                "invalid node_id '{node_id}': hour {hour} out of range 0-23"
            ));
        }
    }
    Ok(())
}
