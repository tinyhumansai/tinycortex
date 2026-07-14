//! Disk-backed episodic capture store.
//!
//! Layout:
//! ```text
//! <content_root>/episodic/<session_id>/<seq:06>.md
//! ```
//!
//! Writes use the same atomic tempfile + rename contract as
//! [`write_if_new`], with
//! one important difference: we want to *append* turns to a session, so the seq
//! is computed from the existing directory contents on each call.
//!
//! This is the per-turn capture surface (one md file per turn, front-matter +
//! body). It is distinct from the batch
//! [`archive_to_tree`](crate::memory::archivist::archive_to_tree) flow, which
//! cleans a whole conversation into a single tree leaf.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

use crate::memory::archivist::types::ArchivedTurn;
use crate::memory::config::MemoryConfig;
use crate::memory::store::content::atomic::write_if_new;

const EPISODIC_DIR: &str = "episodic";

/// Content root for archivist episodic md files, matching the memory-tree
/// content root used elsewhere in the engine.
fn content_root(config: &MemoryConfig) -> PathBuf {
    config.workspace.join("memory_tree").join("content")
}

fn session_dir(config: &MemoryConfig, session_id: &str) -> PathBuf {
    content_root(config)
        .join(EPISODIC_DIR)
        .join(sanitize_session(session_id))
}

/// Map any non-`[A-Za-z0-9_-]` character to `_` so a session id is always a
/// safe single path component.
fn sanitize_session(s: &str) -> String {
    let sanitized: String = s
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if sanitized == s && !sanitized.is_empty() {
        return sanitized;
    }

    // Replacement alone is collision-prone (`a/b` and `a?b`). Keep a short
    // digest of the exact machine id whenever sanitisation changed it.
    let digest = Sha256::digest(s.as_bytes());
    let suffix = digest[..6]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("{sanitized}-{suffix}")
}

/// Next free sequence number for a session: one past the highest `NNNNNN.md`
/// already on disk (or 0 when the directory is empty/missing).
fn next_seq(dir: &Path) -> u32 {
    let mut max = -1i64;
    if let Ok(rd) = fs::read_dir(dir) {
        for entry in rd.flatten() {
            let name = entry.file_name();
            let s = name.to_string_lossy();
            if let Some(stem) = s.strip_suffix(".md") {
                if let Ok(n) = stem.parse::<i64>() {
                    if n > max {
                        max = n;
                    }
                }
            }
        }
    }
    (max + 1) as u32
}

/// Render a turn as a YAML front-matter block followed by its body.
fn compose_turn(turn: &ArchivedTurn) -> String {
    let mut yaml = String::from("---\n");
    yaml.push_str(&format!("session_id: {}\n", yaml_escape(&turn.session_id)));
    yaml.push_str(&format!("seq: {}\n", turn.seq));
    yaml.push_str(&format!("timestamp_ms: {}\n", turn.timestamp_ms));
    yaml.push_str(&format!("role: {}\n", yaml_escape(&turn.role)));
    yaml.push_str(&format!("cost_microdollars: {}\n", turn.cost_microdollars));
    if let Some(lesson) = turn.lesson.as_ref() {
        yaml.push_str(&format!("lesson: {}\n", yaml_escape(lesson)));
    }
    if let Some(tc) = turn.tool_calls_json.as_ref() {
        yaml.push_str(&format!("tool_calls_json: {}\n", yaml_escape(tc)));
    }
    yaml.push_str("---\n\n");
    yaml.push_str(&turn.content);
    if !turn.content.ends_with('\n') {
        yaml.push('\n');
    }
    yaml
}

/// Encode a string as a JSON string literal. JSON quoted scalars are valid
/// YAML, and serde's encoder correctly escapes newlines and every control
/// character that must not appear literally in front matter.
fn yaml_escape(s: &str) -> String {
    serde_json::to_string(s).expect("serializing a string cannot fail")
}

/// Append a turn to its session's archive. Returns the assigned `seq`.
///
/// `turn.seq` is ignored on input — the on-disk directory is the source of
/// truth and the returned [`ArchivedTurn`] carries the actually-assigned seq.
pub fn record_turn(config: &MemoryConfig, mut turn: ArchivedTurn) -> Result<ArchivedTurn> {
    let dir = session_dir(config, &turn.session_id);
    fs::create_dir_all(&dir).with_context(|| format!("failed to mkdir -p {}", dir.display()))?;
    loop {
        turn.seq = next_seq(&dir);
        let path = dir.join(format!("{:06}.md", turn.seq));
        let bytes = compose_turn(&turn).into_bytes();
        match write_if_new(&path, &bytes) {
            Ok(true) => return Ok(turn),
            // Another writer may have claimed the sequence after `next_seq`.
            // `write_if_new` uses create-new semantics, so retrying computes a
            // fresh sequence without ever overwriting the winning turn.
            Ok(false) => continue,
            Err(_) if path.exists() => continue,
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("failed to write episodic turn {}", path.display()))
            }
        }
    }
}

/// Read every turn for `session_id`, sorted by seq ascending. A missing session
/// directory yields an empty vec.
pub fn session_entries(config: &MemoryConfig, session_id: &str) -> Result<Vec<ArchivedTurn>> {
    let dir = session_dir(config, session_id);
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut files: Vec<(u32, PathBuf)> = fs::read_dir(&dir)
        .with_context(|| format!("failed to read_dir {}", dir.display()))?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name();
            let s = name.to_string_lossy();
            let stem = s.strip_suffix(".md")?;
            let seq = stem.parse::<u32>().ok()?;
            Some((seq, e.path()))
        })
        .collect();
    files.sort_by_key(|(seq, _)| *seq);
    let mut out = Vec::with_capacity(files.len());
    for (_, path) in files {
        let bytes =
            fs::read(&path).with_context(|| format!("failed to read {}", path.display()))?;
        let text = String::from_utf8_lossy(&bytes);
        if let Some(turn) = parse_turn(&text) {
            out.push(turn);
        }
    }
    Ok(out)
}

/// Parse a front-matter + body md file back into an [`ArchivedTurn`].
fn parse_turn(text: &str) -> Option<ArchivedTurn> {
    let body_start = text.strip_prefix("---\n")?;
    let end = body_start.find("\n---\n")?;
    let (yaml, rest) = body_start.split_at(end);
    let body = rest.strip_prefix("\n---\n").unwrap_or(rest).to_string();
    let mut turn = ArchivedTurn::default();
    for line in yaml.lines() {
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let k = k.trim();
        let v = v.trim();
        let v_unquoted = serde_json::from_str::<String>(v).unwrap_or_else(|_| v.to_string());
        match k {
            "session_id" => turn.session_id = v_unquoted,
            "seq" => turn.seq = v_unquoted.parse().unwrap_or(0),
            "timestamp_ms" => turn.timestamp_ms = v_unquoted.parse().unwrap_or(0),
            "role" => turn.role = v_unquoted,
            "cost_microdollars" => turn.cost_microdollars = v_unquoted.parse().unwrap_or(0),
            "lesson" => turn.lesson = Some(v_unquoted),
            "tool_calls_json" => turn.tool_calls_json = Some(v_unquoted),
            _ => {}
        }
    }
    // Strip the single blank line compose() writes between the closing `---\n`
    // and the body, then trim the trailing newline. Internal blank lines in the
    // body are preserved.
    turn.content = body
        .strip_prefix('\n')
        .unwrap_or(body.as_str())
        .trim_end()
        .to_string();
    Some(turn)
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
