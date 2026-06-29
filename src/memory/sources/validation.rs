//! Validation rules for memory sources.
//!
//! Two concerns live here:
//!
//! 1. **Configuration validation** — [`validate_entry`] enforces the
//!    required-field rules per [`SourceKind`]. The rules are runtime /
//!    discriminator-based because the kind-specific fields are flattened onto a
//!    single [`MemorySourceEntry`] struct.
//! 2. **Filesystem safety** — [`ensure_within_base`] is the shared
//!    path-traversal defense used by the local readers. Per the engine spec,
//!    "source readers must defend against path traversal".

use std::path::{Path, PathBuf};

use crate::memory::error::{MemoryEngineResult, MemoryError};

use super::types::{MemorySourceEntry, SourceKind};

/// Validate required fields for `entry` based on its [`SourceKind`].
///
/// Returns a human-readable error message describing the first failing rule.
/// `id` and `label` are required for every kind; kind-specific fields follow.
pub fn validate_entry(entry: &MemorySourceEntry) -> Result<(), String> {
    if entry.id.is_empty() {
        return Err("id is required".to_string());
    }
    if entry.label.is_empty() {
        return Err("label is required".to_string());
    }
    match entry.kind {
        SourceKind::Composio => {
            require_field(&entry.toolkit, "toolkit")?;
            require_field(&entry.connection_id, "connection_id")?;
        }
        SourceKind::Conversation => {
            // No kind-specific required fields — just enabled/disabled.
        }
        SourceKind::Folder => {
            require_field(&entry.path, "path")?;
        }
        SourceKind::GithubRepo => {
            require_field(&entry.url, "url")?;
        }
        SourceKind::TwitterQuery => {
            require_field(&entry.query, "query")?;
        }
        SourceKind::RssFeed => {
            require_field(&entry.url, "url")?;
        }
        SourceKind::WebPage => {
            require_field(&entry.url, "url")?;
        }
    }
    Ok(())
}

/// Require that `value` is present and non-empty, naming it `name` in errors.
fn require_field(value: &Option<String>, name: &str) -> Result<(), String> {
    match value {
        Some(v) if !v.is_empty() => Ok(()),
        _ => Err(format!("{name} is required for this source kind")),
    }
}

/// Canonicalize `target` and ensure it stays within canonicalized `base`.
///
/// This is the shared path-traversal guard for local readers. Both paths must
/// exist (they are passed through [`std::fs::canonicalize`], which resolves
/// symlinks and `..` segments). If the resolved target escapes the base
/// directory, a [`MemoryError::PathEscape`] carrying `"path traversal denied"`
/// is returned.
pub fn ensure_within_base(base: &Path, target: &Path) -> MemoryEngineResult<PathBuf> {
    let canonical_base = std::fs::canonicalize(base)?;
    let canonical_target = std::fs::canonicalize(target)?;
    if !canonical_target.starts_with(&canonical_base) {
        return Err(MemoryError::PathEscape("path traversal denied".to_string()));
    }
    Ok(canonical_target)
}

#[cfg(test)]
#[path = "validation_tests.rs"]
mod tests;
