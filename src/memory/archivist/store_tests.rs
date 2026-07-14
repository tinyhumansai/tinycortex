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
fn concurrent_record_turn_retries_sequence_collisions_without_loss() {
    let (_tmp, cfg) = test_config();
    let writers = 24;
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(writers));
    let mut threads = Vec::new();
    for index in 0..writers {
        let cfg = cfg.clone();
        let barrier = barrier.clone();
        threads.push(std::thread::spawn(move || {
            barrier.wait();
            record_turn(&cfg, turn("shared", "user", &format!("turn-{index}"))).unwrap()
        }));
    }
    let mut assigned = Vec::new();
    for thread in threads {
        assigned.push(thread.join().unwrap().seq);
    }
    assigned.sort_unstable();
    assert_eq!(assigned, (0..writers as u32).collect::<Vec<_>>());

    let entries = session_entries(&cfg, "shared").unwrap();
    assert_eq!(entries.len(), writers);
    let contents: std::collections::HashSet<_> =
        entries.into_iter().map(|entry| entry.content).collect();
    assert_eq!(contents.len(), writers);
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
fn front_matter_round_trips_multiline_and_delimiter_like_scalars() {
    let (_tmp, cfg) = test_config();
    let mut t = turn("session:one", "assistant\nadmin", "body stays separate");
    t.lesson = Some("first line\n---\nsecond: line\\tail".into());
    record_turn(&cfg, t.clone()).unwrap();

    let read = session_entries(&cfg, &t.session_id).unwrap();
    assert_eq!(read.len(), 1);
    assert_eq!(read[0].session_id, t.session_id);
    assert_eq!(read[0].role, t.role);
    assert_eq!(read[0].lesson, t.lesson);
    assert_eq!(read[0].content, t.content);
}

#[test]
fn unsafe_session_ids_have_collision_resistant_directories() {
    let (_tmp, cfg) = test_config();
    record_turn(&cfg, turn("a/b", "user", "slash")).unwrap();
    record_turn(&cfg, turn("a?b", "user", "question")).unwrap();

    assert_eq!(session_entries(&cfg, "a/b").unwrap()[0].content, "slash");
    assert_eq!(session_entries(&cfg, "a?b").unwrap()[0].content, "question");
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
