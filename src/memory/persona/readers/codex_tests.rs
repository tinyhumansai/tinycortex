//! Tests for the Codex rollout reader.

use super::*;
use crate::memory::persona::types::EvidenceTier;
use std::io::Write;
use tempfile::TempDir;

fn write_fixture(dir: &TempDir, lines: &[&str]) -> std::path::PathBuf {
    let path = dir.path().join("rollout-2026-07-01T10-00-00-abc.jsonl");
    let mut f = std::fs::File::create(&path).unwrap();
    for l in lines {
        writeln!(f, "{l}").unwrap();
    }
    path
}

const META: &str = r#"{"timestamp":"2026-07-01T10:00:00.000Z","type":"session_meta","payload":{"id":"sess-9","cwd":"/home/droid/work/oh","originator":"codex_exec"}}"#;
const DEVELOPER: &str = r#"{"timestamp":"2026-07-01T10:00:01.000Z","type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"<permissions instructions> you are Codex ..."}]}}"#;
const USER_PROMPT: &str = r#"{"timestamp":"2026-07-01T10:00:02.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"resolve this issue and open a PR"}]}}"#;
const ENV_CTX: &str = r#"{"timestamp":"2026-07-01T10:00:03.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>cwd=/x</environment_context>"}]}}"#;
const SUBAGENT: &str = r#"{"timestamp":"2026-07-01T10:00:04.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<subagent_notification>{\"status\":\"shutdown\"}</subagent_notification>"}]}}"#;
const ASSISTANT: &str = r#"{"timestamp":"2026-07-01T10:00:05.000Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"I will squash all commits into one."}]}}"#;
const CORRECTION: &str = r#"{"timestamp":"2026-07-01T10:00:06.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"stop, keep the commits separate"}]}}"#;
const TOKEN_EVT: &str = r#"{"timestamp":"2026-07-01T10:00:07.000Z","type":"event_msg","payload":{"type":"token_count","total":123}}"#;

#[test]
fn extracts_user_turns_excluding_vendor_and_synthetic() {
    let dir = TempDir::new().unwrap();
    let path = write_fixture(
        &dir,
        &[
            META,
            DEVELOPER,
            USER_PROMPT,
            ENV_CTX,
            SUBAGENT,
            ASSISTANT,
            CORRECTION,
            TOKEN_EVT,
        ],
    );
    let session = read_session(&path).unwrap();

    // Two genuine user turns: the prompt (T2) and the correction (T1).
    assert_eq!(session.evidence.len(), 2);
    assert!(session
        .evidence
        .iter()
        .any(|e| e.excerpt().contains("resolve this issue")));

    let corr = session
        .evidence
        .iter()
        .find(|e| e.tier == EvidenceTier::T1)
        .expect("T1 correction");
    assert!(corr.excerpt().contains("keep the commits separate"));
    assert!(
        corr.excerpt().contains("squash all commits"),
        "carries assistant context"
    );

    // base_instructions (developer), environment_context, and subagent
    // notifications never become evidence.
    assert!(!session
        .evidence
        .iter()
        .any(|e| e.excerpt().contains("Codex")));
    assert!(!session
        .evidence
        .iter()
        .any(|e| e.excerpt().contains("environment_context")));
    assert!(!session
        .evidence
        .iter()
        .any(|e| e.excerpt().contains("subagent_notification")));
}

#[test]
fn provenance_from_session_meta() {
    let dir = TempDir::new().unwrap();
    let path = write_fixture(&dir, &[META, USER_PROMPT]);
    let session = read_session(&path).unwrap();
    assert_eq!(session.source.scope.as_deref(), Some("/home/droid/work/oh"));
    assert_eq!(session.source.session_id.as_deref(), Some("sess-9"));
    assert_eq!(session.source.kind, PersonaSourceKind::Codex);
}

#[test]
fn discover_matches_rollout_prefix_only() {
    let dir = TempDir::new().unwrap();
    write_fixture(&dir, &[META, USER_PROMPT]);
    std::fs::File::create(dir.path().join("notes.jsonl")).unwrap();
    let found = discover(dir.path());
    assert_eq!(found.len(), 1);
}
