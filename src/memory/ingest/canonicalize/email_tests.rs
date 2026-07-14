use super::*;
use chrono::TimeZone;

fn email(ts_ms: i64, from: &str, subject: &str, body: &str) -> EmailMessage {
    EmailMessage {
        from: from.to_string(),
        to: vec!["alice@example.com".into()],
        cc: vec![],
        subject: subject.to_string(),
        sent_at: Utc.timestamp_millis_opt(ts_ms).unwrap(),
        body: body.to_string(),
        source_ref: Some(format!("<msg-{ts_ms}@example.com>")),
        list_unsubscribe: None,
    }
}

#[test]
fn empty_thread_returns_none() {
    let t = EmailThread {
        provider: "gmail".into(),
        thread_subject: "x".into(),
        messages: vec![],
    };
    assert!(canonicalise("gmail:t1", "alice", &[], t).unwrap().is_none());
}

#[test]
fn renders_headers_and_body_per_message() {
    let t = EmailThread {
        provider: "gmail".into(),
        thread_subject: "Launch".into(),
        messages: vec![
            email(1000, "bob@example.com", "Launch", "let's ship"),
            email(2000, "alice@example.com", "Re: Launch", "agreed"),
        ],
    };
    let out = canonicalise(
        "gmail:alice@example.com|bob@example.com",
        "alice@example.com",
        &[],
        t,
    )
    .unwrap()
    .unwrap();
    assert!(
        !out.markdown.contains("# Email thread — gmail — Launch"),
        "canonical email MD must NOT contain a `# ` header"
    );
    assert!(out.markdown.contains("From: bob@example.com"));
    assert!(out.markdown.contains("Subject: Launch"));
    assert!(out.markdown.contains("let's ship"));
    assert!(out.markdown.contains("Re: Launch"));
    assert!(out.markdown.contains("agreed"));
}

#[test]
fn clean_body_strips_footer_before_canonicalise() {
    let body_with_footer =
        "Please review the attached document.\n\nUnsubscribe https://mail.example.com/unsub\n© 2026 Example Corp";
    let t = EmailThread {
        provider: "gmail".into(),
        thread_subject: "Review".into(),
        messages: vec![EmailMessage {
            from: "sender@example.com".into(),
            to: vec!["recipient@example.com".into()],
            cc: vec![],
            subject: "Review".into(),
            sent_at: Utc.timestamp_millis_opt(5000).unwrap(),
            body: body_with_footer.into(),
            source_ref: None,
            list_unsubscribe: None,
        }],
    };
    let out = canonicalise(
        "gmail:recipient@example.com|sender@example.com",
        "recipient@example.com",
        &[],
        t,
    )
    .unwrap()
    .unwrap();
    assert!(
        out.markdown.contains("Please review the attached document"),
        "real content must survive; got:\n{}",
        out.markdown
    );
    assert!(
        !out.markdown.to_ascii_lowercase().contains("unsubscribe"),
        "unsubscribe footer must be stripped; got:\n{}",
        out.markdown
    );
    assert!(
        !out.markdown.contains("© 2026"),
        "copyright footer must be stripped; got:\n{}",
        out.markdown
    );
}

#[test]
fn time_range_spans_thread() {
    let t = EmailThread {
        provider: "gmail".into(),
        thread_subject: "x".into(),
        messages: vec![
            email(3000, "c", "y", "third"),
            email(1000, "a", "y", "first"),
            email(2000, "b", "y", "second"),
        ],
    };
    let out = canonicalise("gmail:t1", "a", &[], t).unwrap().unwrap();
    assert_eq!(out.metadata.time_range.0.timestamp_millis(), 1000);
    assert_eq!(out.metadata.time_range.1.timestamp_millis(), 3000);
}

#[test]
fn source_ref_from_first_message() {
    let t = EmailThread {
        provider: "gmail".into(),
        thread_subject: "x".into(),
        messages: vec![email(1000, "a", "y", "b"), email(2000, "b", "y", "c")],
    };
    let out = canonicalise("gmail:t1", "a", &[], t).unwrap().unwrap();
    assert_eq!(
        out.metadata.source_ref.as_ref().unwrap().value,
        "<msg-1000@example.com>"
    );
}

#[test]
fn blank_source_ref_is_dropped() {
    let mut first = email(1000, "a", "y", "b");
    first.source_ref = Some("".into());
    let t = EmailThread {
        provider: "gmail".into(),
        thread_subject: "x".into(),
        messages: vec![first],
    };
    let out = canonicalise("gmail:t1", "a", &[], t).unwrap().unwrap();
    assert!(out.metadata.source_ref.is_none());
}

// ── Serde regression tests ──────────────────────────────────────────────────

#[test]
fn sent_at_epoch_ms_integer_still_works() {
    let json = r#"{
        "from": "alice@example.com",
        "subject": "Launch",
        "sent_at": 1700000000000,
        "body": "content"
    }"#;
    let msg: EmailMessage = serde_json::from_str(json).expect("epoch-ms integer should parse");
    assert_eq!(msg.sent_at.timestamp_millis(), 1_700_000_000_000);
}

#[test]
fn sent_at_iso8601_string_accepted() {
    let json = r#"{
        "from": "alice@example.com",
        "subject": "Launch",
        "sent_at": "2026-05-17T19:30:00Z",
        "body": "content"
    }"#;
    let msg: EmailMessage = serde_json::from_str(json).expect("ISO-8601 string should parse");
    assert_eq!(msg.sent_at.timestamp(), 1_779_046_200);
}

#[test]
fn sent_at_numeric_string_accepted() {
    let json = r#"{
        "from": "alice@example.com",
        "subject": "Launch",
        "sent_at": "1700000000000",
        "body": "content"
    }"#;
    let msg: EmailMessage = serde_json::from_str(json).expect("numeric string should parse");
    assert_eq!(msg.sent_at.timestamp_millis(), 1_700_000_000_000);
}

#[test]
fn headers_and_body_cannot_inject_email_boundaries() {
    let thread = EmailThread {
        provider: "gmail".into(),
        thread_subject: "thread".into(),
        messages: vec![EmailMessage {
            from: "alice@example.com\n---\nFrom: attacker@example.com".into(),
            to: vec!["bob@example.com\nFrom: forged@example.com".into()],
            cc: vec![],
            subject: "hello\n---\nFrom: forged@example.com".into(),
            sent_at: Utc.timestamp_millis_opt(1_700_000_000_000).unwrap(),
            body: "real body\n---\nFrom: body-forgery@example.com\nmore".into(),
            source_ref: None,
            list_unsubscribe: None,
        }],
    };
    let out = canonicalise("thread", "owner", &[], thread)
        .unwrap()
        .unwrap();
    assert_eq!(
        out.markdown.lines().filter(|line| *line == "---").count(),
        1
    );
    assert!(out
        .markdown
        .contains("\\---\nFrom: body-forgery@example.com"));
}
