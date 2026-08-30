//! Local folder source reader.
//!
//! Walks files under a local directory, matching an optional glob (default
//! `**/*.md`), and reads their content as markdown, HTML, or plaintext.
//!
//! Safety: file sizes are capped at
//! [`FOLDER_FILE_SIZE_CAP_BYTES`]
//! (10 MB) on both list and read, and `read_item` is guarded against path
//! traversal via [`ensure_within_base`].
//!
//! The directory walk uses `walkdir`; glob patterns are compiled to a `regex`
//! (matched against the slash-normalised path relative to the folder root).

use async_trait::async_trait;
use std::path::{Path, PathBuf};

use regex::Regex;
use walkdir::WalkDir;

use crate::memory::config::{MemoryConfig, FOLDER_FILE_SIZE_CAP_BYTES};
use crate::memory::error::{MemoryEngineResult, MemoryError};
use crate::memory::sources::types::{
    ContentType, MemorySourceEntry, SourceContent, SourceItem, SourceKind,
};
use crate::memory::sources::validation::ensure_within_base;

use super::SourceReader;

/// Default glob applied when a folder source does not specify one.
const DEFAULT_GLOB: &str = "**/*.md";

/// Resolve a folder source's configured path against the memory workspace.
///
/// An absolute path is taken verbatim, so every source configured today keeps
/// resolving exactly where it does now. A **relative** path is anchored on
/// [`MemoryConfig::workspace`] — the root this crate already treats as
/// authoritative — instead of being resolved against the process working
/// directory.
///
/// The CWD is not a defensible root for this. It is whatever directory the host
/// process happened to start in; for the OpenHuman desktop app that is the
/// Tauri build directory, so a source configured as `docs` looked in
/// `…/app/src-tauri/docs`, found nothing, and failed on every sync cycle
/// forever (tinyhumansai/openhuman#5830).
fn resolve_base(base_path: &str, workspace: &Path) -> PathBuf {
    let configured = Path::new(base_path);
    if configured.is_absolute() {
        configured.to_path_buf()
    } else {
        workspace.join(configured)
    }
}

/// Build the "folder does not exist" error so it always says **where the reader
/// looked**, not merely what it was configured with.
///
/// Reporting the configured string alone is what made openhuman#5830 cost a
/// source-read and an `lsof` of the running process to diagnose: the log said
/// `folder does not exist: docs` and nothing in it revealed the root that had
/// been joined on. The resolved path is appended only when it differs from the
/// configured one, so an absolute source does not echo itself.
fn missing_folder_error(base_path: &str, resolved: &Path) -> MemoryError {
    let resolved = resolved.display().to_string();
    if resolved == base_path {
        MemoryError::NotFound(format!("folder does not exist: {base_path}"))
    } else {
        MemoryError::NotFound(format!(
            "folder does not exist: {base_path} (resolved to {resolved})"
        ))
    }
}

/// A reader over a local folder of files.
pub struct FolderReader;

#[async_trait]
impl SourceReader for FolderReader {
    fn kind(&self) -> SourceKind {
        SourceKind::Folder
    }

    async fn list_items(
        &self,
        source: &MemorySourceEntry,
        config: &MemoryConfig,
    ) -> MemoryEngineResult<Vec<SourceItem>> {
        let base_path = source
            .path
            .as_deref()
            .ok_or_else(|| MemoryError::Invalid("folder source requires a path".to_string()))?;
        let pattern = source.glob.as_deref().unwrap_or(DEFAULT_GLOB);

        let base = resolve_base(base_path, &config.workspace);
        if !base.exists() {
            return Err(missing_folder_error(base_path, &base));
        }

        let matcher = glob_to_regex(pattern)?;

        let mut items = Vec::new();
        for entry in WalkDir::new(&base).follow_links(false) {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            let rel = match path.strip_prefix(&base) {
                Ok(r) => r,
                Err(_) => continue,
            };
            let rel_str = normalize_rel(rel);
            if !matcher.is_match(&rel_str) {
                continue;
            }
            let metadata = match std::fs::metadata(path) {
                Ok(m) => m,
                Err(_) => continue,
            };
            if metadata.len() > FOLDER_FILE_SIZE_CAP_BYTES {
                continue;
            }
            let modified_ms = metadata
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64);

            items.push(SourceItem {
                id: rel_str.clone(),
                title: rel_str,
                updated_at_ms: modified_ms,
            });
        }

        Ok(items)
    }

    /// Read one file's content by its `item_id` (the slash-normalised path
    /// relative to the source's `path`, as produced by
    /// [`list_items`](Self::list_items)).
    ///
    async fn read_item(
        &self,
        source: &MemorySourceEntry,
        item_id: &str,
        config: &MemoryConfig,
    ) -> MemoryEngineResult<SourceContent> {
        let base_path = source
            .path
            .as_deref()
            .ok_or_else(|| MemoryError::Invalid("folder source requires a path".to_string()))?;

        let pattern = source.glob.as_deref().unwrap_or(DEFAULT_GLOB);
        let matcher = glob_to_regex(pattern)?;
        let normalized_id = normalize_rel(Path::new(item_id));
        if !matcher.is_match(&normalized_id) {
            return Err(MemoryError::Invalid(format!(
                "item '{item_id}' is outside source glob '{pattern}'"
            )));
        }

        // Resolve through the same rule `list_items` used, so a relative source
        // reads back the files it listed. Splitting these would be worse than
        // the bug: list would walk the workspace while read looked in the CWD.
        let base = resolve_base(base_path, &config.workspace);
        let file_path = base.join(item_id);
        if !file_path.exists() {
            return Err(MemoryError::NotFound(format!(
                "file not found: {}",
                file_path.display()
            )));
        }

        // Canonicalize and verify the resolved file stays within the folder
        // root — defends against `..` traversal and symlink escapes.
        // Containment is checked against the *resolved* base. Passing the raw
        // configured string here would canonicalise a relative base against the
        // CWD while `file_path` sits under the workspace, so the two roots
        // would not correspond — the check has to see the same base the file
        // was joined onto.
        let canonical_file = ensure_within_base(&base, &file_path)?;

        // Apply the same size cap as list_items so a huge file can't blow up
        // the renderer or the chunker.
        let metadata = std::fs::metadata(&canonical_file)?;
        if metadata.len() > FOLDER_FILE_SIZE_CAP_BYTES {
            return Err(MemoryError::Invalid(format!(
                "file exceeds {FOLDER_FILE_SIZE_CAP_BYTES}-byte limit: {}",
                canonical_file.display()
            )));
        }

        let body = std::fs::read_to_string(&canonical_file)?;

        let content_type = if item_id.ends_with(".md") {
            ContentType::Markdown
        } else if item_id.ends_with(".html") || item_id.ends_with(".htm") {
            ContentType::Html
        } else {
            ContentType::Plaintext
        };

        Ok(SourceContent {
            id: item_id.to_string(),
            title: item_id.to_string(),
            body,
            content_type,
            metadata: serde_json::json!({}),
        })
    }
}

/// Normalise a relative path to forward slashes for glob matching.
fn normalize_rel(rel: &Path) -> String {
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Compile a shell-style glob into an anchored [`Regex`] matched against a
/// slash-normalised relative path.
///
/// Supported syntax: `*` (any run of non-separator chars), `?` (one
/// non-separator char), `**` (any run including separators), and `**/` (zero or
/// more leading directories). All other regex metacharacters are escaped.
fn glob_to_regex(pattern: &str) -> MemoryEngineResult<Regex> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut re = String::from("^");
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '*' => {
                if i + 1 < chars.len() && chars[i + 1] == '*' {
                    if i + 2 < chars.len() && chars[i + 2] == '/' {
                        // `**/` — zero or more leading directories.
                        re.push_str("(?:.*/)?");
                        i += 3;
                    } else {
                        // `**` — any run including separators.
                        re.push_str(".*");
                        i += 2;
                    }
                } else {
                    // `*` — any run excluding separators.
                    re.push_str("[^/]*");
                    i += 1;
                }
            }
            '?' => {
                re.push_str("[^/]");
                i += 1;
            }
            '/' => {
                re.push('/');
                i += 1;
            }
            '.' | '+' | '(' | ')' | '|' | '^' | '$' | '{' | '}' | '[' | ']' | '\\' => {
                re.push('\\');
                re.push(c);
                i += 1;
            }
            other => {
                re.push(other);
                i += 1;
            }
        }
    }
    re.push('$');
    Regex::new(&re).map_err(|e| MemoryError::Invalid(format!("invalid glob pattern: {e}")))
}

#[cfg(test)]
#[path = "folder_tests.rs"]
mod tests;
