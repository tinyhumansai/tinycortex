//! Tests for the Claude Code transcript reader.

use super::*;
use crate::memory::persona::types::EvidenceTier;
use std::io::Write;
use tempfile::TempDir;

/// Write a `.jsonl` fixture and return its path (kept alive by `dir`).
fn write_fixture(dir: &TempDir, name: &str, lines: &[&str]) -> std::path::PathBuf {
    let proj = dir.path().join("-home-droid-work-demo");
    std::fs::create_dir_all(&proj).unwrap();
    let path = proj.join(name);
    let mut f = std::fs::File::create(&path).unwrap();
    for l in lines {
        writeln!(f, "{l}").unwrap();
    }
    path
}

const REAL_PROMPT: &str = r#"{"type":"user","isSidechain":false,"cwd":"/home/droid/work/demo","sessionId":"sess-1","timestamp":"2026-07-01T10:00:00.000Z","message":{"role":"user","content":"implement the parser and add a regression test"}}"#;
const CORRECTION: &str = r#"{"type":"user","isSidechain":false,"sessionId":"sess-1","timestamp":"2026-07-01T10:05:00.000Z","message":{"role":"user","content":"no, use a streaming approach instead"}}"#;
const ASSISTANT: &str = r#"{"type":"assistant","timestamp":"2026-07-01T10:04:00.000Z","message":{"role":"assistant","content":[{"type":"thinking","thinking":"hmm"},{"type":"text","text":"I loaded the whole file into memory."}]}}"#;
const TOOL_RESULT: &str = r#"{"type":"user","isSidechain":false,"message":{"role":"user","content":[{"type":"tool_result","content":"stdout blob"}]}}"#;
const SIDECHAIN: &str = r#"{"type":"user","isSidechain":true,"message":{"role":"user","content":"subagent prompt"}}"#;
const META: &str = r#"{"type":"user","isMeta":true,"message":{"role":"user","content":"meta note"}}"#;
const CAVEAT: &str = r#"{"type":"user","isSidechain":false,"message":{"role":"user","content":"<local-command-caveat>Caveat...</local-command-caveat>"}}"#;
const REMINDER: &str = r#"{"type":"user","isSidechain":false,"timestamp":"2026-07-01T10:06:00.000Z","message":{"role":"user","content":"ship it <system-reminder>background noise</system-reminder>"}}"#;

#[test]
fn extracts_only_user_authored_turns() {
    let dir = TempDir::new().unwrap();
    let path = write_fixture(
        &dir,
        "abc.jsonl",
        &[
            REAL_PROMPT, ASSISTANT, CORRECTION, TOOL_RESULT, SIDECHAIN, META, CAVEAT, REMINDER,
        ],
    );
    let session = read_session(&path).unwrap();

    // Four genuine user turns survive: prompt, correction, and the reminder-
    // wrapped "ship it". (tool_result, sidechain, meta, caveat all dropped.)
    let excerpts: Vec<&str> = session.evidence.iter().map(|e| e.excerpt()).collect();
    assert_eq!(session.evidence.len(), 3, "got: {excerpts:?}");

    // Correction is tiered T1 and carries the preceding assistant context.
    let corr = session
        .evidence
        .iter()
        .find(|e| e.tier == EvidenceTier::T1)
        .expect("a T1 correction");
    assert!(corr.excerpt().contains("streaming approach"));
    assert!(
        corr.excerpt().contains("whole file into memory"),
        "correction should carry assistant context: {}",
        corr.excerpt()
    );

    // system-reminder scaffolding is stripped from the kept prompt.
    assert!(session
        .evidence
        .iter()
        .any(|e| e.excerpt().contains("ship it") && !e.excerpt().contains("background noise")));
}

#[test]
fn provenance_prefers_cwd_and_session_id() {
    let dir = TempDir::new().unwrap();
    let path = write_fixture(&dir, "abc.jsonl", &[REAL_PROMPT]);
    let session = read_session(&path).unwrap();
    assert_eq!(session.source.scope.as_deref(), Some("/home/droid/work/demo"));
    assert_eq!(session.source.session_id.as_deref(), Some("sess-1"));
}

#[test]
fn tool_result_heavy_session_reduces_over_95_percent() {
    let dir = TempDir::new().unwrap();
    // One short prompt amidst a pile of big tool-result blobs.
    let big = format!(
        r#"{{"type":"user","isSidechain":false,"message":{{"role":"user","content":[{{"type":"tool_result","content":"{}"}}]}}}}"#,
        "x".repeat(5000)
    );
    let mut lines: Vec<&str> = vec![REAL_PROMPT];
    for _ in 0..20 {
        lines.push(&big);
    }
    let path = write_fixture(&dir, "abc.jsonl", &lines);
    let session = read_session(&path).unwrap();
    assert!(
        session.reduction_ratio() > 0.95,
        "reduction was only {:.3}",
        session.reduction_ratio()
    );
}

#[test]
fn discover_finds_jsonl_recursively() {
    let dir = TempDir::new().unwrap();
    write_fixture(&dir, "one.jsonl", &[REAL_PROMPT]);
    write_fixture(&dir, "two.jsonl", &[REAL_PROMPT]);
    let found = discover(dir.path());
    assert_eq!(found.len(), 2);
}
