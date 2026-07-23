use super::*;
use chrono::TimeZone;

fn msg(ts_ms: i64, author: &str, text: &str) -> ChatMessage {
    ChatMessage {
        author: author.to_string(),
        timestamp: Utc.timestamp_millis_opt(ts_ms).unwrap(),
        text: text.to_string(),
        source_ref: Some(format!("slack://x/{ts_ms}")),
    }
}

#[test]
fn empty_batch_returns_none() {
    let b = ChatBatch {
        platform: "slack".into(),
        channel_label: "#eng".into(),
        messages: vec![],
    };
    assert!(canonicalise("slack:#eng", "alice", &[], b)
        .unwrap()
        .is_none());
}

#[test]
fn messages_are_sorted_and_range_captured() {
    let b = ChatBatch {
        platform: "slack".into(),
        channel_label: "#eng".into(),
        messages: vec![
            msg(2000, "bob", "second"),
            msg(1000, "alice", "first"),
            msg(3000, "carol", "third"),
        ],
    };
    let out = canonicalise("slack:#eng", "alice", &["eng".into()], b)
        .unwrap()
        .unwrap();
    assert_eq!(out.metadata.time_range.0.timestamp_millis(), 1000);
    assert_eq!(out.metadata.time_range.1.timestamp_millis(), 3000);
    let pos_first = out.markdown.find("first").unwrap();
    let pos_second = out.markdown.find("second").unwrap();
    let pos_third = out.markdown.find("third").unwrap();
    assert!(pos_first < pos_second);
    assert!(pos_second < pos_third);
}

#[test]
fn includes_per_message_sections_without_header() {
    let b = ChatBatch {
        platform: "slack".into(),
        channel_label: "#eng".into(),
        messages: vec![msg(1000, "alice", "hello")],
    };
    let out = canonicalise("slack:#eng", "alice", &[], b)
        .unwrap()
        .unwrap();
    assert!(
        !out.markdown.starts_with("# "),
        "canonical chat MD must NOT start with a `# ` header"
    );
    assert!(
        out.markdown.starts_with("## "),
        "must start with first `## ` message block"
    );
    assert!(out.markdown.contains("— alice"));
    assert!(out.markdown.contains("hello"));
}

#[test]
fn source_ref_taken_from_first_message() {
    let b = ChatBatch {
        platform: "slack".into(),
        channel_label: "#eng".into(),
        messages: vec![msg(1000, "alice", "hi"), msg(2000, "bob", "hey")],
    };
    let out = canonicalise("slack:#eng", "alice", &[], b)
        .unwrap()
        .unwrap();
    assert_eq!(
        out.metadata.source_ref.as_ref().unwrap().value,
        "slack://x/1000"
    );
}

#[test]
fn metadata_carries_owner_and_tags() {
    let b = ChatBatch {
        platform: "slack".into(),
        channel_label: "#eng".into(),
        messages: vec![msg(1000, "alice", "hi")],
    };
    let out = canonicalise(
        "slack:#eng",
        "alice@example.com",
        &["eng".into(), "on-call".into()],
        b,
    )
    .unwrap()
    .unwrap();
    assert_eq!(out.metadata.owner, "alice@example.com");
    assert_eq!(out.metadata.tags, vec!["eng", "on-call"]);
    assert_eq!(out.metadata.source_kind, SourceKind::Chat);
}

#[test]
fn blank_source_ref_is_dropped() {
    let mut first = msg(1000, "alice", "hi");
    first.source_ref = Some("   ".into());
    let b = ChatBatch {
        platform: "slack".into(),
        channel_label: "#eng".into(),
        messages: vec![first],
    };
    let out = canonicalise("slack:#eng", "alice", &[], b)
        .unwrap()
        .unwrap();
    assert!(out.metadata.source_ref.is_none());
}

// ── Serde regression tests ──────────────────────────────────────────────────

#[test]
fn timestamp_epoch_ms_integer_still_works() {
    let json = r#"{
        "author": "alice",
        "timestamp": 1700000000000,
        "text": "hello"
    }"#;
    let msg: ChatMessage = serde_json::from_str(json).expect("epoch-ms integer should parse");
    assert_eq!(msg.timestamp.timestamp_millis(), 1_700_000_000_000);
}

#[test]
fn timestamp_iso8601_string_accepted() {
    let json = r#"{
        "author": "alice",
        "timestamp": "2026-05-17T19:30:00Z",
        "text": "hello"
    }"#;
    let msg: ChatMessage = serde_json::from_str(json).expect("ISO-8601 string should parse");
    assert_eq!(msg.timestamp.timestamp(), 1_779_046_200);
}

#[test]
fn timestamp_numeric_string_accepted() {
    let json = r#"{
        "author": "alice",
        "timestamp": "1700000000000",
        "text": "hello"
    }"#;
    let msg: ChatMessage = serde_json::from_str(json).expect("numeric string should parse");
    assert_eq!(msg.timestamp.timestamp_millis(), 1_700_000_000_000);
}

#[test]
fn timestamp_epoch_seconds_are_rejected_instead_of_treated_as_millis() {
    for timestamp in [
        serde_json::json!(1_700_000_000),
        serde_json::json!("1700000000"),
    ] {
        let value = serde_json::json!({
            "author": "alice",
            "text": "hello",
            "timestamp": timestamp,
        });
        let err = serde_json::from_value::<ChatMessage>(value).unwrap_err();
        assert!(err.to_string().contains("milliseconds, not seconds"));
    }
}

/// Regression #5169: a payload with no `timestamp` field must deserialize
/// gracefully (defaulting to `Utc::now()`) instead of failing with "missing
/// field `timestamp`".
#[test]
fn missing_timestamp_defaults_to_now() {
    let json = r#"{
        "author": "alice",
        "text": "hello"
    }"#;
    let msg: ChatMessage = serde_json::from_str(json).expect("missing timestamp should not fail");
    let diff = Utc::now().signed_duration_since(msg.timestamp);
    assert!(
        diff.num_seconds().unsigned_abs() < 5,
        "defaulted timestamp should be within ~5s of now, got {diff:?}"
    );
}

/// Regression #5169: null timestamp should also fall back to the default.
#[test]
fn null_timestamp_defaults_to_now() {
    let json = r#"{
        "author": "alice",
        "timestamp": null,
        "text": "hello"
    }"#;
    let msg: ChatMessage = serde_json::from_str(json).expect("null timestamp should not fail");
    let diff = Utc::now().signed_duration_since(msg.timestamp);
    assert!(
        diff.num_seconds().unsigned_abs() < 5,
        "defaulted timestamp should be within ~5s of now, got {diff:?}"
    );
}

#[test]
fn message_content_cannot_inject_chat_boundaries() {
    let batch = ChatBatch {
        platform: "slack".into(),
        channel_label: "eng".into(),
        messages: vec![ChatMessage {
            author: "alice\n## forged-author".into(),
            timestamp: Utc.timestamp_millis_opt(1_700_000_000_000).unwrap(),
            text: "hello\n## forged message\nbye".into(),
            source_ref: None,
        }],
    };
    let out = canonicalise("channel", "owner", &[], batch)
        .unwrap()
        .unwrap();
    assert_eq!(
        out.markdown
            .lines()
            .filter(|line| line.starts_with("## "))
            .count(),
        1
    );
    assert!(out.markdown.contains("\\## forged message"));
}
