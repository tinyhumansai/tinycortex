//! Local folder source reader.
//!
//! Walks files under a local directory, matching an optional glob (default
//! `**/*.md`), and reads their content as markdown, HTML, or plaintext.
//!
//! Safety: file sizes are capped at
//! [`FOLDER_FILE_SIZE_CAP_BYTES`](crate::memory::config::FOLDER_FILE_SIZE_CAP_BYTES)
//! (10 MB) on both list and read, and `read_item` is guarded against path
//! traversal via [`ensure_within_base`](crate::memory::sources::validation::ensure_within_base).
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
        _config: &MemoryConfig,
    ) -> MemoryEngineResult<Vec<SourceItem>> {
        let base_path = source
            .path
            .as_deref()
            .ok_or_else(|| MemoryError::Invalid("folder source requires a path".to_string()))?;
        let pattern = source.glob.as_deref().unwrap_or(DEFAULT_GLOB);

        let base = PathBuf::from(base_path);
        if !base.exists() {
            return Err(MemoryError::NotFound(format!(
                "folder does not exist: {base_path}"
            )));
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

    async fn read_item(
        &self,
        source: &MemorySourceEntry,
        item_id: &str,
        _config: &MemoryConfig,
    ) -> MemoryEngineResult<SourceContent> {
        let base_path = source
            .path
            .as_deref()
            .ok_or_else(|| MemoryError::Invalid("folder source requires a path".to_string()))?;

        let file_path = Path::new(base_path).join(item_id);
        if !file_path.exists() {
            return Err(MemoryError::NotFound(format!(
                "file not found: {}",
                file_path.display()
            )));
        }

        // Canonicalize and verify the resolved file stays within the folder
        // root — defends against `..` traversal and symlink escapes.
        let canonical_file = ensure_within_base(Path::new(base_path), &file_path)?;

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
