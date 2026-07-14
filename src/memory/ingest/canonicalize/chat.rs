//! Chat transcripts → canonical Markdown.
//!
//! Chat sources are scoped by **channel or group**. A batch of chat messages
//! from the same channel becomes one [`CanonicalisedSource`]; the chunker
//! slices it by token budget downstream.
//!
//! Output format (no leading `# ...` header — that info lives in front-matter;
//! the chunker splits at `## ` boundaries):
//! ```md
//! ## 2026-04-21T10:12:00Z — Alice
//! Message body here.
//!
//! ## 2026-04-21T10:12:40Z — Bob
//! Reply body here.
//! ```
//!
//! Header newlines are collapsed, and body lines beginning with `## ` are
//! escaped so untrusted content cannot forge the chunker's message boundary.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{normalize_source_ref, CanonicalisedSource};
use crate::memory::chunks::{Metadata, SourceKind};

/// One chat message in a channel/group.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatMessage {
    /// Author display name or id.
    pub author: String,
    /// When the message was sent (epoch-ms integer or RFC 3339 string).
    #[serde(
        serialize_with = "chrono::serde::ts_milliseconds::serialize",
        deserialize_with = "super::deserialize_flexible_timestamp"
    )]
    pub timestamp: DateTime<Utc>,
    /// Plain text / markdown body.
    pub text: String,
    /// Optional per-message provenance pointer (permalink or `platform://...`).
    #[serde(default)]
    pub source_ref: Option<String>,
}

/// Adapter input — a batch of messages from one logical channel.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatBatch {
    /// Platform name used in the header (e.g. `slack`, `discord`, `telegram`).
    pub platform: String,
    /// Human-readable channel / group name for the header.
    pub channel_label: String,
    /// Ordered messages (chronological; adapter sorts defensively).
    pub messages: Vec<ChatMessage>,
}

/// Canonicalise a chat batch.
///
/// Returns `Ok(None)` if the batch has zero messages — callers treat that as
/// "nothing to ingest" and skip.
pub fn canonicalise(
    source_id: &str,
    owner: &str,
    tags: &[String],
    batch: ChatBatch,
) -> Result<Option<CanonicalisedSource>, String> {
    if batch.messages.is_empty() {
        return Ok(None);
    }
    let mut messages = batch.messages;
    messages.sort_by_key(|m| m.timestamp);

    let first_ts = messages.first().map(|m| m.timestamp).unwrap();
    let last_ts = messages.last().map(|m| m.timestamp).unwrap();

    let mut md = String::new();
    // No leading `# Chat transcript — ...` header. Platform / channel info
    // belongs in the MD front-matter. The chunker splits this output at `## `
    // boundaries so each message becomes one chunk.
    for msg in &messages {
        let author = msg.author.replace(['\n', '\r'], " ");
        let body = msg
            .text
            .trim()
            .lines()
            .map(|line| {
                if line.starts_with("## ") {
                    format!("\\{line}")
                } else {
                    line.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        md.push_str(&format!(
            "## {} — {}\n{}\n\n",
            msg.timestamp.to_rfc3339(),
            author,
            body
        ));
    }

    // Provenance points at the batch's first message by default (or whatever
    // the caller passed on the first message).
    let source_ref = normalize_source_ref(messages.first().and_then(|m| m.source_ref.clone()));

    let metadata = Metadata {
        source_kind: SourceKind::Chat,
        source_id: source_id.to_string(),
        owner: owner.to_string(),
        timestamp: first_ts,
        time_range: (first_ts, last_ts),
        tags: tags.to_vec(),
        source_ref,
        path_scope: None,
    };
    Ok(Some(CanonicalisedSource {
        markdown: md,
        metadata,
    }))
}

#[cfg(test)]
#[path = "chat_tests.rs"]
mod tests;
