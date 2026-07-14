//! Claude Code transcript reader (doc 06 §6.4, Family A).
//!
//! Discovers `~/.claude/projects/<project-slug>/<uuid>.jsonl` event streams and
//! extracts **user-authored turns only**: genuine prompts (T2), corrections /
//! interrupts (T1), and slash-command habits (T2). It drops the machine —
//! assistant prose, tool results, reasoning, sidechains (`isSidechain`), meta
//! turns (`isMeta`), and system-reminder / local-command scaffolding — which is
//! the ≥95% byte reduction the persona pipeline relies on.

use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::{DateTime, Utc};
use walkdir::WalkDir;

use super::super::types::{EvidenceSource, EvidenceTier, PersonaEvidence, PersonaSourceKind};
use super::{jsonl, looks_like_correction, RawSession};

/// Max chars of preceding assistant text carried into a correction excerpt so
/// the digest model can see what was being corrected (§6.4).
const ASSISTANT_CONTEXT_CHARS: usize = 200;

/// Discover every Claude Code transcript file under `root` (typically
/// `~/.claude/projects`). Returns paths sorted for deterministic ordering.
pub fn discover(root: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = WalkDir::new(root)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.into_path())
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .collect();
    files.sort();
    files
}

/// Parse one Claude Code transcript file into a [`RawSession`].
pub fn read_session(path: &Path) -> Result<RawSession> {
    let mut source = EvidenceSource::new(PersonaSourceKind::ClaudeCode)
        .with_path(path.to_string_lossy().to_string());
    // Provisional scope from the project-slug directory; refined by `cwd` below.
    if let Some(slug) = path.parent().and_then(|p| p.file_name()) {
        source = source.with_scope(slug.to_string_lossy().to_string());
    }
    if let Some(stem) = path.file_stem() {
        source = source.with_session(stem.to_string_lossy().to_string());
    }

    let mut session = RawSession::new(source);
    let mut last_assistant: Option<String> = None;
    let mut cwd_scope: Option<String> = None;
    let mut session_id: Option<String> = None;
    let mut pending: Vec<(DateTime<Utc>, EvidenceTier, String)> = Vec::new();

    let raw_bytes = jsonl::for_each_json_line(path, |_len, v| {
        let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        // Learn provenance from any event that carries it.
        if cwd_scope.is_none() {
            if let Some(cwd) = v.get("cwd").and_then(|c| c.as_str()) {
                cwd_scope = Some(cwd.to_string());
            }
        }
        if session_id.is_none() {
            if let Some(sid) = v.get("sessionId").and_then(|c| c.as_str()) {
                session_id = Some(sid.to_string());
            }
        }

        match ty {
            "assistant" => {
                if let Some(text) = assistant_text(v) {
                    last_assistant = Some(text);
                }
            }
            "user" => {
                // Drop sidechains (subagent traffic) and meta turns outright.
                if v.get("isSidechain").and_then(|b| b.as_bool()).unwrap_or(false) {
                    return;
                }
                if v.get("isMeta").and_then(|b| b.as_bool()).unwrap_or(false) {
                    return;
                }
                let content = match v.get("message").and_then(|m| m.get("content")) {
                    Some(c) => c,
                    None => return,
                };
                let text = match user_text(content) {
                    Some(t) => t,
                    None => return, // synthetic tool-result turn or empty
                };
                let ts = event_timestamp(v).unwrap_or_else(Utc::now);
                let (tier, excerpt) = if looks_like_correction(&text) {
                    (EvidenceTier::T1, with_assistant_context(&last_assistant, &text))
                } else {
                    (EvidenceTier::T2, format!("user: {text}"))
                };
                pending.push((ts, tier, excerpt));
            }
            _ => {}
        }
    })?;

    // Prefer the real cwd as scope; keep the session id.
    if let Some(cwd) = cwd_scope {
        session.source = session.source.with_scope(cwd);
    }
    if let Some(sid) = session_id {
        session.source = session.source.with_session(sid);
    }
    let src = session.source.clone();
    for (ts, tier, excerpt) in pending {
        session.push(PersonaEvidence::new(src.clone(), ts, tier, &excerpt, vec![]));
    }
    session.raw_bytes = raw_bytes;
    Ok(session)
}

/// Parse an event's RFC3339 `timestamp` field into UTC.
fn event_timestamp(v: &serde_json::Value) -> Option<DateTime<Utc>> {
    let s = v.get("timestamp").and_then(|t| t.as_str())?;
    DateTime::parse_from_rfc3339(s).ok().map(|t| t.with_timezone(&Utc))
}

/// Extract the leading assistant text (first [`ASSISTANT_CONTEXT_CHARS`] chars),
/// ignoring thinking / tool-use blocks. `None` if the turn has no text.
fn assistant_text(v: &serde_json::Value) -> Option<String> {
    let content = v.get("message").and_then(|m| m.get("content"))?;
    let text = collect_text_blocks(content);
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    Some(text.chars().take(ASSISTANT_CONTEXT_CHARS).collect())
}

/// Prefix a correction excerpt with the preceding assistant snippet so the
/// digest can see what was being corrected.
fn with_assistant_context(last_assistant: &Option<String>, user_text: &str) -> String {
    match last_assistant {
        Some(ctx) if !ctx.trim().is_empty() => {
            format!("user (interrupting agent that said \"{ctx}\"): {user_text}")
        }
        _ => format!("user (interrupt): {user_text}"),
    }
}

/// Extract genuine user text from a `message.content`, or `None` if the turn is
/// synthetic (tool-result) or empty after cleaning.
fn user_text(content: &serde_json::Value) -> Option<String> {
    let raw = match content {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(_) => {
            // A list with any tool_result block is a synthetic user turn.
            if has_block_type(content, "tool_result") {
                return None;
            }
            collect_text_blocks(content)
        }
        _ => return None,
    };
    let cleaned = clean_user_text(&raw);
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

/// True if a content block array contains a block of `kind`.
fn has_block_type(content: &serde_json::Value, kind: &str) -> bool {
    content
        .as_array()
        .map(|arr| {
            arr.iter()
                .any(|b| b.get("type").and_then(|t| t.as_str()) == Some(kind))
        })
        .unwrap_or(false)
}

/// Concatenate the `text` fields of any `text`-typed blocks in a content array.
fn collect_text_blocks(content: &serde_json::Value) -> String {
    match content {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(arr) => arr
            .iter()
            .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
            .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Strip harness scaffolding (system reminders, local-command wrappers) and
/// return the human-authored remainder. Empty string means "not user text".
fn clean_user_text(raw: &str) -> String {
    let s = raw.trim();
    // Local-command / caveat wrappers are execution scaffolding, not prompts.
    if s.starts_with("<local-command")
        || s.starts_with("<command-name")
        || s.starts_with("<command-message")
        || s.starts_with("Caveat:")
        || s.starts_with("<bash-")
    {
        return String::new();
    }
    // Remove any embedded <system-reminder>…</system-reminder> spans.
    let without_reminders = strip_tag_spans(s, "system-reminder");
    without_reminders.trim().to_string()
}

/// Remove every `<tag>…</tag>` span (case-sensitive) from `text`.
fn strip_tag_spans(text: &str, tag: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(&open) {
        out.push_str(&rest[..start]);
        if let Some(end) = rest[start..].find(&close) {
            rest = &rest[start + end + close.len()..];
        } else {
            rest = &rest[start + open.len()..];
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
#[path = "claude_code_tests.rs"]
mod tests;
