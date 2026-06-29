use super::*;
use chrono::TimeZone;

fn doc(title: &str, body: &str) -> DocumentInput {
    DocumentInput {
        provider: "notion".into(),
        title: title.into(),
        body: body.into(),
        modified_at: Utc.timestamp_millis_opt(1_700_000_000_000).unwrap(),
        source_ref: Some("notion://page/abc".into()),
    }
}

#[test]
fn empty_doc_returns_none() {
    let d = DocumentInput {
        provider: "notion".into(),
        title: "".into(),
        body: "   \n  ".into(),
        modified_at: Utc::now(),
        source_ref: None,
    };
    assert!(canonicalise("d1", "alice", &[], d, None).unwrap().is_none());
}

#[test]
fn renders_body_without_header() {
    let out = canonicalise(
        "d1",
        "alice",
        &[],
        doc("Launch plan", "step one\n\nstep two"),
        None,
    )
    .unwrap()
    .unwrap();
    assert!(
        !out.markdown.starts_with("# "),
        "canonical document MD must NOT start with a `# ` header"
    );
    assert!(out.markdown.contains("step one"));
    assert!(out.markdown.contains("step two"));
}

#[test]
fn metadata_single_point_time_range() {
    let out = canonicalise("d1", "alice", &[], doc("x", "y"), None)
        .unwrap()
        .unwrap();
    assert_eq!(out.metadata.time_range.0, out.metadata.time_range.1);
    assert_eq!(out.metadata.source_kind, SourceKind::Document);
}

#[test]
fn source_ref_carried_through() {
    let out = canonicalise("d1", "alice", &["proj".into()], doc("x", "y"), None)
        .unwrap()
        .unwrap();
    assert_eq!(
        out.metadata.source_ref.as_ref().unwrap().value,
        "notion://page/abc"
    );
    assert_eq!(out.metadata.tags, vec!["proj"]);
}

#[test]
fn blank_source_ref_is_dropped() {
    let mut input = doc("x", "y");
    input.source_ref = Some(" \n ".into());
    let out = canonicalise("d1", "alice", &[], input, None)
        .unwrap()
        .unwrap();
    assert!(out.metadata.source_ref.is_none());
}

// ── Serde regression / fix tests ─────────────────────────────────────────────

#[test]
fn modified_at_epoch_ms_integer_still_works() {
    let json = r#"{
        "provider": "notion",
        "title": "My doc",
        "body": "content",
        "modified_at": 1700000000000
    }"#;
    let input: DocumentInput = serde_json::from_str(json).expect("epoch-ms integer should parse");
    assert_eq!(
        input.modified_at.timestamp_millis(),
        1_700_000_000_000,
        "epoch-ms round-trip"
    );
}

#[test]
fn modified_at_iso8601_string_accepted() {
    let json = r#"{
        "provider": "drive",
        "title": "Meeting notes",
        "body": "agenda here",
        "modified_at": "2026-05-17T19:30:00Z"
    }"#;
    let input: DocumentInput = serde_json::from_str(json).expect("ISO-8601 string should parse");
    assert_eq!(input.modified_at.timestamp(), 1_779_046_200);
}

#[test]
fn modified_at_missing_defaults_to_now() {
    let before = Utc::now();
    let json = r#"{
        "provider": "notion",
        "title": "No timestamp doc",
        "body": "body text"
    }"#;
    let input: DocumentInput =
        serde_json::from_str(json).expect("missing modified_at should parse");
    let after = Utc::now();
    assert!(
        input.modified_at >= before && input.modified_at <= after,
        "default modified_at {ts} must fall between {before} and {after}",
        ts = input.modified_at,
    );
}

#[test]
fn provider_missing_defaults_to_unknown() {
    let json = r#"{
        "title": "No provider doc",
        "body": "body text",
        "modified_at": 1700000000000
    }"#;
    let input: DocumentInput = serde_json::from_str(json).expect("missing provider should parse");
    assert_eq!(input.provider, "unknown");
}
