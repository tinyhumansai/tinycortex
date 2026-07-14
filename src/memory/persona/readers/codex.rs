//! Codex rollout transcript reader (doc 06 §6.4, Family A).
//!
//! Discovers `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` and extracts
//! user-authored turns from `response_item` `message` events with
//! `role == "user"`, dropping the `developer` role (permissions /
//! `base_instructions` — vendor prompts) and synthetic wrappers
//! (`<environment_context>`, `<user_instructions>`, `<subagent_notification>`,
//! `<permissions …>`). Session provenance (cwd, id) comes from `session_meta`.

use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::{DateTime, Utc};
use walkdir::WalkDir;

use super::super::types::{EvidenceSource, EvidenceTier, PersonaEvidence, PersonaSourceKind};
use super::{jsonl, looks_like_correction, RawSession};

const ASSISTANT_CONTEXT_CHARS: usize = 200;

/// Discover every Codex rollout file under `root` (typically
/// `~/.codex/sessions`). Sorted for deterministic ordering.
pub fn discover(root: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = WalkDir::new(root)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.into_path())
        .filter(|p| {
            p.extension().is_some_and(|x| x == "jsonl")
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("rollout-"))
        })
        .collect();
    files.sort();
    files
}

/// Parse one Codex rollout file into a [`RawSession`].
pub fn read_session(path: &Path) -> Result<RawSession> {
    let mut source =
        EvidenceSource::new(PersonaSourceKind::Codex).with_path(path.to_string_lossy().to_string());
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
        let payload = v.get("payload");

        if ty == "session_meta" {
            if let Some(p) = payload {
                if let Some(cwd) = p.get("cwd").and_then(|c| c.as_str()) {
                    cwd_scope = Some(cwd.to_string());
                }
                if let Some(id) = p.get("id").and_then(|c| c.as_str()) {
                    session_id = Some(id.to_string());
                }
            }
            return;
        }

        if ty != "response_item" {
            return;
        }
        let p = match payload {
            Some(p) if p.get("type").and_then(|t| t.as_str()) == Some("message") => p,
            _ => return,
        };
        let role = p.get("role").and_then(|r| r.as_str()).unwrap_or("");
        match role {
            "assistant" => {
                let text = collect_message_text(p);
                let text = text.trim();
                if !text.is_empty() {
                    last_assistant = Some(text.chars().take(ASSISTANT_CONTEXT_CHARS).collect());
                }
            }
            "user" => {
                let raw = collect_message_text(p);
                let text = clean_user_text(&raw);
                if text.is_empty() {
                    return;
                }
                let ts = event_timestamp(v).unwrap_or_else(Utc::now);
                let (tier, excerpt) = if looks_like_correction(&text) {
                    (
                        EvidenceTier::T1,
                        with_assistant_context(&last_assistant, &text),
                    )
                } else {
                    (EvidenceTier::T2, format!("user: {text}"))
                };
                pending.push((ts, tier, excerpt));
            }
            _ => {} // developer / tool / system — vendor scaffolding
        }
    })?;

    if let Some(cwd) = cwd_scope {
        session.source = session.source.with_scope(cwd);
    }
    if let Some(sid) = session_id {
        session.source = session.source.with_session(sid);
    }
    let src = session.source.clone();
    for (ts, tier, excerpt) in pending {
        session.push(PersonaEvidence::new(
            src.clone(),
            ts,
            tier,
            &excerpt,
            vec![],
        ));
    }
    session.raw_bytes = raw_bytes;
    Ok(session)
}

fn event_timestamp(v: &serde_json::Value) -> Option<DateTime<Utc>> {
    let s = v.get("timestamp").and_then(|t| t.as_str())?;
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

fn with_assistant_context(last_assistant: &Option<String>, user_text: &str) -> String {
    match last_assistant {
        Some(ctx) if !ctx.trim().is_empty() => {
            format!("user (interrupting agent that said \"{ctx}\"): {user_text}")
        }
        _ => format!("user (interrupt): {user_text}"),
    }
}

/// Concatenate `input_text` / `output_text` / `text` blocks of a Codex message.
fn collect_message_text(msg: &serde_json::Value) -> String {
    let content = match msg.get("content").and_then(|c| c.as_array()) {
        Some(a) => a,
        None => return String::new(),
    };
    content
        .iter()
        .filter_map(|b| {
            let t = b.get("type").and_then(|t| t.as_str()).unwrap_or("");
            if matches!(t, "input_text" | "output_text" | "text") {
                b.get("text").and_then(|t| t.as_str())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Strip Codex synthetic wrappers; empty string means "not user text".
fn clean_user_text(raw: &str) -> String {
    let s = raw.trim();
    if s.starts_with("<environment_context>")
        || s.starts_with("<user_instructions>")
        || s.starts_with("<subagent_notification>")
        || s.starts_with("<permissions")
        || s.is_empty()
    {
        return String::new();
    }
    s.to_string()
}

#[cfg(test)]
#[path = "codex_tests.rs"]
mod tests;
