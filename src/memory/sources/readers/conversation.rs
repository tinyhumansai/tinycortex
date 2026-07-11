//! Conversation source reader.
//!
//! Treats every agent conversation thread as a memory source item. Threads are
//! JSON files under `<workspace>/threads/`; when synced, each thread's messages
//! are rendered to markdown and stored as durable memory alongside other
//! sources.
//!
//! Safety: `item_id` is rejected if it contains path separators or `..`, and the
//! resolved file is re-checked for containment within the threads directory.

use async_trait::async_trait;

use crate::memory::config::MemoryConfig;
use crate::memory::error::{MemoryEngineResult, MemoryError};
use crate::memory::sources::types::{
    ContentType, MemorySourceEntry, SourceContent, SourceItem, SourceKind,
};
use crate::memory::sources::validation::ensure_within_base;

use super::SourceReader;

/// A reader over local agent conversation threads.
pub struct ConversationReader;

#[async_trait]
impl SourceReader for ConversationReader {
    fn kind(&self) -> SourceKind {
        SourceKind::Conversation
    }

    async fn list_items(
        &self,
        _source: &MemorySourceEntry,
        config: &MemoryConfig,
    ) -> MemoryEngineResult<Vec<SourceItem>> {
        let threads_dir = config.workspace.join("threads");
        if !threads_dir.exists() {
            return Ok(Vec::new());
        }

        let mut items = Vec::new();
        for entry in std::fs::read_dir(&threads_dir)? {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let id = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();

            let modified_ms = entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64);

            items.push(SourceItem {
                title: id.clone(),
                id,
                updated_at_ms: modified_ms,
            });
        }

        Ok(items)
    }

    /// Read one thread's content by its `item_id` (the thread's file stem, as
    /// produced by [`list_items`](Self::list_items)).
    ///
    /// NOTE: the traversal guard below rejects any `item_id` containing the
    /// substring `".."`, not just path-separator-adjacent `..` segments. A
    /// thread file whose stem legitimately contains `".."` (e.g.
    /// `standup..2026.json`, which [`list_items`](Self::list_items) itself
    /// would happily produce as an id) is therefore unreadable even though it
    /// poses no traversal risk once separators are excluded.
    async fn read_item(
        &self,
        _source: &MemorySourceEntry,
        item_id: &str,
        config: &MemoryConfig,
    ) -> MemoryEngineResult<SourceContent> {
        // Validate item_id to prevent path traversal before touching the FS.
        if item_id.contains("..") || item_id.contains('/') || item_id.contains('\\') {
            return Err(MemoryError::Invalid(
                "invalid item_id: path traversal denied".to_string(),
            ));
        }

        let threads_dir = config.workspace.join("threads");
        let thread_path = threads_dir.join(format!("{item_id}.json"));

        if !thread_path.exists() {
            return Err(MemoryError::NotFound(format!(
                "thread '{item_id}' not found"
            )));
        }

        // Re-check containment after resolving symlinks.
        ensure_within_base(&threads_dir, &thread_path)?;

        let raw = std::fs::read_to_string(&thread_path)?;
        let parsed: serde_json::Value = serde_json::from_str(&raw)?;

        let title = parsed
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or(item_id)
            .to_string();

        let body = format_thread_as_markdown(&parsed);

        Ok(SourceContent {
            id: item_id.to_string(),
            title,
            body,
            content_type: ContentType::Markdown,
            metadata: serde_json::json!({
                "source_type": "conversation",
                "thread_id": item_id,
            }),
        })
    }
}

/// Render a thread JSON value (`{ title, messages: [{ role, content }] }`) to
/// markdown. Messages with empty content are skipped.
fn format_thread_as_markdown(thread: &serde_json::Value) -> String {
    let mut out = String::new();

    if let Some(title) = thread.get("title").and_then(|v| v.as_str()) {
        out.push_str(&format!("# {title}\n\n"));
    }

    let messages = thread
        .get("messages")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    for msg in &messages {
        let role = msg
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let content = msg.get("content").and_then(|v| v.as_str()).unwrap_or("");

        if content.is_empty() {
            continue;
        }

        out.push_str(&format!("**{role}**: {content}\n\n"));
    }

    out
}

#[cfg(test)]
#[path = "conversation_tests.rs"]
mod tests;
