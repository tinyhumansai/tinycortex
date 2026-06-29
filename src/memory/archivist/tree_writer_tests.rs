//! Tests for the end-to-end archive flow. Ported from OpenHuman's inline
//! `memory_archivist::tree_writer` tests, rewritten against the injected
//! [`RecordingSink`] instead of a live `memory_tree` so the archivist stays
//! decoupled from tree internals.

use super::{archive_to_tree, chunk_id_for_session};
use crate::memory::archivist::sink::RecordingSink;
use crate::memory::archivist::types::Turn;
use chrono::TimeZone;

#[test]
fn chunk_id_is_stable_for_same_session_and_markdown() {
    let a = chunk_id_for_session("session-1", "## user\nhello\n");
    let b = chunk_id_for_session("session-1", "## user\nhello\n");
    assert_eq!(a, b);
    assert!(a.starts_with("archivist:"));
}

#[test]
fn chunk_id_changes_when_session_changes() {
    let a = chunk_id_for_session("session-1", "## user\nhello\n");
    let b = chunk_id_for_session("session-2", "## user\nhello\n");
    assert_ne!(a, b);
}

#[test]
fn chunk_id_changes_when_markdown_changes() {
    let a = chunk_id_for_session("session-1", "## user\nhello\n");
    let b = chunk_id_for_session("session-1", "## user\nhello again\n");
    assert_ne!(a, b);
}

#[test]
fn archive_to_tree_writes_one_cleaned_leaf_for_conversation_turns() {
    let sink = RecordingSink::new();
    let turns = vec![
        Turn {
            role: "user".into(),
            content: "How does ownership work in Rust?".into(),
            tool_calls_json: None,
            timestamp: chrono::Utc.with_ymd_and_hms(2026, 5, 24, 10, 0, 0).unwrap(),
        },
        Turn {
            role: "assistant".into(),
            content: "Ownership gives each value a single owner.".into(),
            // tool_calls_json must be stripped before archival.
            tool_calls_json: Some("{\"tool\":\"ignored\"}".into()),
            timestamp: chrono::Utc.with_ymd_and_hms(2026, 5, 24, 10, 1, 0).unwrap(),
        },
        Turn {
            // a tool-result turn — dropped entirely.
            role: "tool".into(),
            content: "{\"stdout\":\"noise that should never embed\"}".into(),
            tool_calls_json: None,
            timestamp: chrono::Utc.with_ymd_and_hms(2026, 5, 24, 10, 2, 0).unwrap(),
        },
    ];

    let outcome = archive_to_tree(&sink, "session-1", &turns).expect("archive_to_tree");
    assert!(
        outcome.new_summary_ids.is_empty(),
        "single archivist leaf should not seal summaries immediately"
    );
    assert!(!outcome.seal_pending);

    // Exactly one leaf was appended.
    let leaves = sink.leaves();
    assert_eq!(leaves.len(), 1);
    let leaf = &leaves[0];

    // The tool turn is gone and the tool_calls_json is stripped.
    let expected_md = "## user\nHow does ownership work in Rust?\n\n## assistant\nOwnership gives each value a single owner.\n";
    assert_eq!(leaf.markdown, expected_md);
    assert!(!leaf.markdown.contains("tool"));
    assert!(!leaf.markdown.contains("ignored"));

    // Metadata: deterministic chunk id, session provenance, token heuristic.
    assert_eq!(
        leaf.meta.chunk_id,
        chunk_id_for_session("session-1", expected_md)
    );
    assert_eq!(outcome.chunk_id, leaf.meta.chunk_id);
    assert_eq!(leaf.meta.session_id, "session-1");
    assert_eq!(leaf.meta.token_count, (expected_md.len() / 4).max(1) as u32);
    // Leaf timestamp follows the last *cleaned* turn (the assistant), not the
    // dropped tool turn.
    assert_eq!(
        leaf.meta.timestamp,
        chrono::Utc.with_ymd_and_hms(2026, 5, 24, 10, 1, 0).unwrap()
    );
}

#[test]
fn archive_to_tree_handles_empty_turns_via_fallback_markdown() {
    let sink = RecordingSink::new();
    let outcome = archive_to_tree(&sink, "session-empty", &[]).expect("archive_to_tree empty");
    assert!(outcome.new_summary_ids.is_empty());

    let leaves = sink.leaves();
    assert_eq!(leaves.len(), 1);
    assert_eq!(leaves[0].markdown, "");
    assert_eq!(
        leaves[0].meta.chunk_id,
        chunk_id_for_session("session-empty", ""),
        "empty conversation still generates a deterministic archivist chunk id"
    );
    // Empty markdown still costs at least one token.
    assert_eq!(leaves[0].meta.token_count, 1);
}

#[test]
fn archive_to_tree_accumulates_multiple_sessions_in_call_order() {
    let sink = RecordingSink::new();
    let mut expected_ids = Vec::new();
    for idx in 0..3 {
        let turns = vec![Turn {
            role: "user".into(),
            content: format!("Conversation {idx} about the phoenix rollout."),
            tool_calls_json: None,
            timestamp: chrono::Utc
                .with_ymd_and_hms(2026, 5, 24, 10, idx, 0)
                .unwrap(),
        }];
        let outcome = archive_to_tree(&sink, &format!("session-{idx}"), &turns)
            .expect("archive_to_tree multi-session batch");
        assert!(outcome.new_summary_ids.is_empty());

        let expected_md = format!("## user\nConversation {idx} about the phoenix rollout.\n");
        expected_ids.push(chunk_id_for_session(
            &format!("session-{idx}"),
            &expected_md,
        ));
    }

    let leaves = sink.leaves();
    let got_ids: Vec<String> = leaves.iter().map(|l| l.meta.chunk_id.clone()).collect();
    assert_eq!(got_ids, expected_ids);
    assert_eq!(leaves.len(), 3);
}

#[test]
fn archive_to_tree_propagates_sealed_summary_ids_from_sink() {
    let sink = RecordingSink::with_seal_ids(vec!["sum:1".into(), "sum:2".into()]);
    let turns = vec![Turn::new("user", "anything")];
    let outcome = archive_to_tree(&sink, "session-x", &turns).expect("archive_to_tree");
    assert_eq!(outcome.new_summary_ids, vec!["sum:1", "sum:2"]);
    assert!(!outcome.seal_pending);
}
