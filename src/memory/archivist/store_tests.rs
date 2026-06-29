//! Tests for the disk-backed episodic capture store. Ported from OpenHuman's
//! inline `memory_archivist::store` tests, adapted to TinyCortex's
//! [`MemoryConfig`].

use super::*;
use tempfile::TempDir;

fn test_config() -> (TempDir, MemoryConfig) {
    let tmp = TempDir::new().unwrap();
    let cfg = MemoryConfig::new(tmp.path().to_path_buf());
    (tmp, cfg)
}

fn turn(session: &str, role: &str, content: &str) -> ArchivedTurn {
    ArchivedTurn {
        session_id: session.into(),
        seq: 0,
        timestamp_ms: 1_700_000_000_000,
        role: role.into(),
        content: content.into(),
        lesson: None,
        tool_calls_json: None,
        cost_microdollars: 0,
    }
}

#[test]
fn round_trip_single_turn() {
    let (_tmp, cfg) = test_config();
    let stored = record_turn(&cfg, turn("s1", "user", "hello world")).unwrap();
    assert_eq!(stored.seq, 0);
    let read = session_entries(&cfg, "s1").unwrap();
    assert_eq!(read.len(), 1);
    assert_eq!(read[0].content, "hello world");
    assert_eq!(read[0].role, "user");
    assert_eq!(read[0].session_id, "s1");
    assert_eq!(read[0].seq, 0);
}

#[test]
fn append_increments_seq() {
    let (_tmp, cfg) = test_config();
    let a = record_turn(&cfg, turn("s1", "user", "one")).unwrap();
    let b = record_turn(&cfg, turn("s1", "assistant", "two")).unwrap();
    let c = record_turn(&cfg, turn("s1", "user", "three")).unwrap();
    assert_eq!((a.seq, b.seq, c.seq), (0, 1, 2));
    let read = session_entries(&cfg, "s1").unwrap();
    assert_eq!(
        read.iter().map(|t| t.seq).collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    assert_eq!(read[1].role, "assistant");
    assert_eq!(read[2].content, "three");
}

#[test]
fn missing_session_returns_empty() {
    let (_tmp, cfg) = test_config();
    assert!(session_entries(&cfg, "never").unwrap().is_empty());
}

#[test]
fn preserves_lesson_and_tool_calls() {
    let (_tmp, cfg) = test_config();
    let mut t = turn("s1", "assistant", "did the thing");
    t.lesson = Some("be careful with X: it bites".into());
    t.tool_calls_json = Some(r#"[{"name":"bash","args":{"cmd":"ls"}}]"#.into());
    t.cost_microdollars = 1234;
    record_turn(&cfg, t.clone()).unwrap();
    let read = session_entries(&cfg, "s1").unwrap();
    assert_eq!(
        read[0].lesson.as_deref(),
        Some("be careful with X: it bites")
    );
    assert_eq!(
        read[0].tool_calls_json.as_deref(),
        Some(r#"[{"name":"bash","args":{"cmd":"ls"}}]"#)
    );
    assert_eq!(read[0].cost_microdollars, 1234);
}

#[test]
fn distinct_sessions_dont_mix() {
    let (_tmp, cfg) = test_config();
    record_turn(&cfg, turn("a", "user", "hi a")).unwrap();
    record_turn(&cfg, turn("b", "user", "hi b")).unwrap();
    record_turn(&cfg, turn("a", "user", "more a")).unwrap();
    let a = session_entries(&cfg, "a").unwrap();
    let b = session_entries(&cfg, "b").unwrap();
    assert_eq!(a.len(), 2);
    assert_eq!(b.len(), 1);
    assert_eq!(b[0].content, "hi b");
}
