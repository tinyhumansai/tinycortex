//! Email threads → canonical Markdown.
//!
//! Email sources are scoped by **participant set**. One participant bucket
//! becomes one [`CanonicalisedSource`]. Headers (From, To, Cc, Subject, Date)
//! surface in a small frontmatter-style block per message; the cleaned body
//! follows as markdown. Bodies pass through [`email_clean::clean_body`] before
//! rendering to strip reply chains, marketing footers, legal disclaimers, and
//! other boilerplate.
//!
//! Header values collapse newlines through [`email_clean::md_escape`], and
//! body lines equal to `---` are escaped so untrusted content cannot forge the
//! downstream email boundary grammar.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{email_clean, normalize_source_ref, CanonicalisedSource};
use crate::memory::chunks::{Metadata, SourceKind};

/// One email in a thread.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EmailMessage {
    /// Sender address; rendered as the `From:` header and used as the
    /// participant key when bucketing a thread.
    pub from: String,
    /// Primary recipient addresses; rendered as the `To:` header (omitted when empty).
    #[serde(default)]
    pub to: Vec<String>,
    /// Carbon-copy recipient addresses; rendered as the `Cc:` header (omitted when empty).
    #[serde(default)]
    pub cc: Vec<String>,
    /// Per-message subject; rendered as the `Subject:` header.
    pub subject: String,
    /// When the message was sent (epoch-ms integer or RFC 3339 string).
    #[serde(
        serialize_with = "chrono::serde::ts_milliseconds::serialize",
        deserialize_with = "super::deserialize_flexible_timestamp"
    )]
    pub sent_at: DateTime<Utc>,
    /// Plain-text or markdown body.
    pub body: String,
    /// Message-id header or provider URL; used for citation back to source.
    #[serde(default)]
    pub source_ref: Option<String>,
    /// List-Unsubscribe header for one-click unsubscribe actions.
    #[serde(default)]
    pub list_unsubscribe: Option<String>,
}

/// A whole email thread.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EmailThread {
    /// Provider name used in the header (e.g. `gmail`, `outlook`).
    pub provider: String,
    /// Thread subject shown on top (usually the subject of the first message).
    pub thread_subject: String,
    /// Ordered messages (chronological; adapter sorts defensively).
    pub messages: Vec<EmailMessage>,
}

/// Canonicalise an email thread into a [`CanonicalisedSource`]. Bodies are
/// passed through [`email_clean::clean_body`] to strip reply chains and footer
/// boilerplate. Returns `Ok(None)` when the thread has no messages.
pub fn canonicalise(
    source_id: &str,
    owner: &str,
    tags: &[String],
    thread: EmailThread,
) -> Result<Option<CanonicalisedSource>, String> {
    if thread.messages.is_empty() {
        return Ok(None);
    }
    let mut messages = thread.messages;
    messages.sort_by_key(|m| m.sent_at);

    let first_ts = messages.first().map(|m| m.sent_at).unwrap();
    let last_ts = messages.last().map(|m| m.sent_at).unwrap();

    let mut md = String::new();
    // No leading `# Email thread — ...` header. Provider / subject info belongs
    // in the MD front-matter. The chunker splits this output at `---\nFrom:`
    // boundaries so each message becomes one chunk.
    for msg in &messages {
        md.push_str("---\n");
        md.push_str(&format!("From: {}\n", email_clean::md_escape(&msg.from)));
        if !msg.to.is_empty() {
            md.push_str(&format!(
                "To: {}\n",
                email_clean::md_escape(&msg.to.join(", "))
            ));
        }
        if !msg.cc.is_empty() {
            md.push_str(&format!(
                "Cc: {}\n",
                email_clean::md_escape(&msg.cc.join(", "))
            ));
        }
        md.push_str(&format!(
            "Subject: {}\n",
            email_clean::md_escape(&msg.subject)
        ));
        md.push_str(&format!("Date: {}\n", msg.sent_at.to_rfc3339()));

        if let Some(unsub) = &msg.list_unsubscribe {
            md.push_str(&format!(
                "List-Unsubscribe: {}\n",
                email_clean::md_escape(unsub)
            ));
        }
        md.push('\n');
        let cleaned = email_clean::clean_body(msg.body.trim());
        if cleaned.is_empty() {
            md.push('\n');
        } else {
            let safe_body = cleaned
                .lines()
                .map(|line| {
                    if line.trim_end() == "---" {
                        format!("\\{line}")
                    } else {
                        line.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            md.push_str(&safe_body);
        }
        md.push_str("\n\n");
    }

    let source_ref = normalize_source_ref(messages.first().and_then(|m| m.source_ref.clone()));

    Ok(Some(CanonicalisedSource {
        markdown: md,
        metadata: Metadata {
            source_kind: SourceKind::Email,
            source_id: source_id.to_string(),
            owner: owner.to_string(),
            timestamp: first_ts,
            time_range: (first_ts, last_ts),
            tags: tags.to_vec(),
            source_ref,
            path_scope: None,
        },
    }))
}

#[cfg(test)]
#[path = "email_tests.rs"]
mod tests;
