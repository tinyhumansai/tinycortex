use crate::memory::chunks::{Chunk, Metadata, SourceKind, SourceRef};
use crate::memory::store::content::compose::chunk::{compose_chunk_file, rewrite_tags};
use crate::memory::store::content::compose::summary::{
    compose_summary_md, rewrite_summary_tags, scope_short_label, SummaryComposeInput,
};
use crate::memory::store::content::compose::yaml::{
    scan_fm_field, split_front_matter, yaml_scalar,
};
use crate::memory::store::content::compose::{MEMORY_ARTIFACT_FORMAT, OPENHUMAN_CORE_VERSION};
use crate::memory::store::content::paths::SummaryTreeKind;
use chrono::TimeZone;

fn sample_chunk() -> Chunk {
    let ts = chrono::Utc.timestamp_millis_opt(1_700_000_000_000).unwrap();
    Chunk {
        id: "abc123".into(),
        content: "## 2026-01-01T00:00:00Z — alice\nhello world".into(),
        metadata: Metadata {
            source_kind: SourceKind::Chat,
            source_id: "slack:#eng".into(),
            owner: "alice@example.com".into(),
            timestamp: ts,
            time_range: (ts, ts),
            tags: vec!["person/Alice".into(), "org/Acme".into()],
            source_ref: Some(SourceRef::new("slack://m1".to_string())),
            path_scope: None,
        },
        token_count: 10,
        seq_in_source: 0,
        created_at: ts,
        partial_message: false,
    }
}

#[test]
fn compose_produces_front_matter_and_body() {
    let chunk = sample_chunk();
    let (full, body) = compose_chunk_file(&chunk);
    let full_str = std::str::from_utf8(&full).unwrap();
    assert!(full_str.starts_with("---\n"), "must start with ---");
    assert!(full_str.contains("source_kind: chat"));
    assert!(full_str.contains("source_id: \"slack:#eng\""));
    assert!(full_str.contains("seq: 0"));
    assert!(full_str.contains("tags:"));
    assert!(full_str.contains("  - person/Alice"));
    assert!(full_str.ends_with("hello world"));
    assert_eq!(
        body,
        b"## 2026-01-01T00:00:00Z \xe2\x80\x94 alice\nhello world"
    );
}

#[test]
fn compose_persists_path_scope_and_seeds_scoped_source_tag() {
    let mut chunk = sample_chunk();
    chunk.metadata.source_id = "notion:conn-1:page-123".into();
    chunk.metadata.path_scope = Some("notion:conn-1".into());

    let (full, _) = compose_chunk_file(&chunk);
    let full_str = std::str::from_utf8(&full).unwrap();

    assert!(full_str.contains("source_id: \"notion:conn-1:page-123\""));
    assert!(full_str.contains("path_scope: \"notion:conn-1\""));
    assert!(full_str.contains("  - source/notion-conn-1"));
    assert!(!full_str.contains("  - source/notion-conn-1-page-123"));
}

#[test]
fn split_front_matter_round_trips() {
    let chunk = sample_chunk();
    let (full, body) = compose_chunk_file(&chunk);
    let full_str = std::str::from_utf8(&full).unwrap();
    let (fm, b) = split_front_matter(full_str).expect("split must succeed");
    assert!(fm.starts_with("---\n"));
    assert!(fm.ends_with("---\n"));
    assert_eq!(b.as_bytes(), body.as_slice());
}

#[test]
fn rewrite_tags_preserves_body() {
    let chunk = sample_chunk();
    let (full, body) = compose_chunk_file(&chunk);
    let new_tags = vec!["person/Bob".into(), "project/Phoenix".into()];
    let rewritten = rewrite_tags(&full, &new_tags).unwrap();
    let rewritten_str = std::str::from_utf8(&rewritten).unwrap();
    assert!(rewritten_str.contains("  - person/Bob"));
    assert!(!rewritten_str.contains("  - person/Alice"));
    assert!(rewritten_str.ends_with(std::str::from_utf8(&body).unwrap()));
}

#[test]
fn rewrite_tags_empty_list() {
    let chunk = sample_chunk();
    let (full, _) = compose_chunk_file(&chunk);
    let rewritten = rewrite_tags(&full, &[]).unwrap();
    let s = std::str::from_utf8(&rewritten).unwrap();
    assert!(s.contains("tags: []"));
    assert!(!s.contains("  - person/"));
}

#[test]
fn yaml_scalar_quotes_special_characters() {
    assert_eq!(yaml_scalar("slack:#eng"), "\"slack:#eng\"");
    assert_eq!(yaml_scalar("hello world"), "hello world");
    assert_eq!(yaml_scalar(""), "\"\"");
}

// ─── frontmatter-yaml hardening (SC-1, SC-2, SC-19) ───────────────────────

/// SC-1: a file whose closing fence is `\n---` at EOF (no trailing newline,
/// empty body) must parse instead of panicking on an out-of-bounds slice.
#[test]
fn split_front_matter_handles_eof_fence_without_trailing_newline() {
    let content = "---\nsource_id: foo\n---";
    let (fm, body) = split_front_matter(content).expect("eof fence must parse");
    assert_eq!(fm, "---\nsource_id: foo\n---");
    assert_eq!(body, "");
}

/// SC-1: genuinely malformed front matter returns `None`, never panics.
#[test]
fn split_front_matter_malformed_returns_none() {
    assert!(split_front_matter("---\n").is_none());
    assert!(split_front_matter("---\nno closing fence").is_none());
    assert!(split_front_matter("not front matter").is_none());
    // Empty front matter, EOF fence.
    let (fm, body) = split_front_matter("---\n\n---").expect("empty fm parses");
    assert_eq!(fm, "---\n\n---");
    assert_eq!(body, "");
}

/// SC-2: `yaml_scalar` must quote values with control characters and escape
/// newlines/tabs so provider values cannot inject front-matter lines.
#[test]
fn yaml_scalar_escapes_control_characters() {
    assert_eq!(yaml_scalar("a\nb"), "\"a\\nb\"");
    assert_eq!(yaml_scalar("a\tb"), "\"a\\tb\"");
    assert_eq!(yaml_scalar("a\rb"), "\"a\\rb\"");
    // A value that tries to terminate the block must be neutralised.
    assert_eq!(yaml_scalar("x\n---\n"), "\"x\\n---\\n\"");
}

/// SC-2: a provider value carrying `\n---\n` must round-trip through
/// compose -> split -> scan without corrupting the body or injecting lines.
#[test]
fn adversarial_newline_value_round_trips_with_body_intact() {
    let mut chunk = sample_chunk();
    let evil = "evil\n---\nowner: attacker\ntags:\n  - injected";
    chunk.metadata.source_id = evil.into();

    let (full, body) = compose_chunk_file(&chunk);
    let full_str = std::str::from_utf8(&full).unwrap();

    let (fm, b) = split_front_matter(full_str).expect("must split");
    // Body bytes are exactly the original content — the injection did not
    // shift the closing fence.
    assert_eq!(b.as_bytes(), body.as_slice());
    assert_eq!(b.as_bytes(), chunk.content.as_bytes());
    // The value is recovered verbatim, decoded back from its escaped form.
    let recovered = scan_fm_field(fm, "source_id").expect("source_id present");
    assert_eq!(recovered, evil);
    // The injection is confined to a single escaped line: no standalone
    // `owner: attacker` line leaked, and `owner` still resolves to the real
    // metadata owner rather than the attacker-supplied value.
    assert!(fm.lines().all(|l| l.trim() != "owner: attacker"));
    assert_eq!(
        scan_fm_field(fm, "owner").as_deref(),
        Some("alice@example.com")
    );
}

/// SC-19: a value containing `\"` (backslash then quote) must round-trip —
/// the unescape order must not corrupt it.
#[test]
fn backslash_quote_value_round_trips() {
    let mut chunk = sample_chunk();
    let raw = r#"a\"b\\c"#; // a \ " b \ \ c
    chunk.metadata.source_id = raw.into();

    let (full, _body) = compose_chunk_file(&chunk);
    let full_str = std::str::from_utf8(&full).unwrap();
    let (fm, _) = split_front_matter(full_str).expect("must split");
    let recovered = scan_fm_field(fm, "source_id").expect("source_id present");
    assert_eq!(recovered, raw);
}

fn sample_email_chunk() -> Chunk {
    let ts = chrono::Utc.timestamp_millis_opt(1_700_000_000_000).unwrap();
    Chunk {
        id: "emailchunk1".into(),
        content: "---\nFrom: alice@example.com\nSubject: Hello\n\nHello there.".into(),
        metadata: Metadata {
            source_kind: SourceKind::Email,
            source_id: "gmail:alice@example.com|bob@example.com".into(),
            owner: "owner@example.com".into(),
            timestamp: ts,
            time_range: (ts, ts),
            tags: vec!["gmail".into()],
            source_ref: None,
            path_scope: None,
        },
        token_count: 15,
        seq_in_source: 0,
        created_at: ts,
        partial_message: false,
    }
}

#[test]
fn email_chunk_has_participants_list_and_alias() {
    let chunk = sample_email_chunk();
    let (full, _body) = compose_chunk_file(&chunk);
    let full_str = std::str::from_utf8(&full).unwrap();
    assert!(full_str.contains("participants:"));
    assert!(full_str.contains("  - alice@example.com"));
    assert!(full_str.contains("  - bob@example.com"));
    assert!(full_str.contains("aliases:"));
    assert!(full_str.contains("alice@example.com <-> bob@example.com: chunk 0"));
    assert!(!full_str.contains("sender:"));
    assert!(!full_str.contains("thread_id:"));
}

#[test]
fn email_chunk_many_participants_alias_summarises() {
    let ts = chrono::Utc.timestamp_millis_opt(1_700_000_000_000).unwrap();
    let chunk = Chunk {
        id: "em2".into(),
        content: "body".into(),
        metadata: Metadata {
            source_kind: SourceKind::Email,
            source_id: "gmail:alice@x.com|bob@y.com|carol@z.com".into(),
            owner: "owner".into(),
            timestamp: ts,
            time_range: (ts, ts),
            tags: vec![],
            source_ref: None,
            path_scope: None,
        },
        token_count: 1,
        seq_in_source: 3,
        created_at: ts,
        partial_message: false,
    };
    let (full, _) = compose_chunk_file(&chunk);
    let full_str = std::str::from_utf8(&full).unwrap();
    assert!(full_str.contains("participants:"));
    assert!(full_str.contains("alice@x.com <-> 2 others: chunk 3"));
}

#[test]
fn email_chunk_body_bytes_unchanged_by_extra_fields() {
    let chunk = sample_email_chunk();
    let (full, body) = compose_chunk_file(&chunk);
    let full_str = std::str::from_utf8(&full).unwrap();
    assert!(full_str.ends_with(std::str::from_utf8(&body).unwrap()));
    assert_eq!(body, chunk.content.as_bytes());
}

#[test]
fn chat_chunk_has_no_email_specific_fields() {
    let chunk = sample_chunk();
    let (full, _) = compose_chunk_file(&chunk);
    let full_str = std::str::from_utf8(&full).unwrap();
    assert!(!full_str.contains("aliases:"));
    assert!(!full_str.contains("participants:"));
    assert!(!full_str.contains("sender:"));
    assert!(!full_str.contains("thread_id:"));
}

#[test]
fn email_chunk_with_malformed_source_id_omits_extra_fields() {
    let ts = chrono::Utc.timestamp_millis_opt(1_700_000_000_000).unwrap();
    let chunk = Chunk {
        id: "xyz".into(),
        content: "body".into(),
        metadata: Metadata {
            source_kind: SourceKind::Email,
            source_id: "legacysourceid".into(),
            owner: "owner".into(),
            timestamp: ts,
            time_range: (ts, ts),
            tags: vec![],
            source_ref: None,
            path_scope: None,
        },
        token_count: 1,
        seq_in_source: 0,
        created_at: ts,
        partial_message: false,
    };
    let (full, _) = compose_chunk_file(&chunk);
    let full_str = std::str::from_utf8(&full).unwrap();
    assert!(!full_str.contains("aliases:"));
    assert!(!full_str.contains("participants:"));
    assert!(!full_str.contains("sender:"));
}

// ─── summary compose tests ────────────────────────────────────────────────

fn sample_summary_input(
    tree_kind: SummaryTreeKind,
    scope: &str,
    level: u32,
) -> SummaryComposeInput<'static> {
    let ts_start = chrono::Utc.timestamp_millis_opt(1_700_000_000_000).unwrap();
    let ts_end = chrono::Utc.timestamp_millis_opt(1_700_086_400_000).unwrap();
    let sealed = chrono::Utc.timestamp_millis_opt(1_700_090_000_000).unwrap();
    let scope: &'static str = Box::leak(scope.to_string().into_boxed_str());
    SummaryComposeInput {
        summary_id: "summary:L1:abc",
        tree_kind,
        tree_id: "tree-id-001",
        tree_scope: scope,
        level,
        child_ids: Box::leak(vec!["child-1".to_string(), "child-2".to_string()].into_boxed_slice()),
        child_basenames: None,
        child_count: 2,
        time_range_start: ts_start,
        time_range_end: ts_end,
        sealed_at: sealed,
        body: "This is the summariser output.\n",
    }
}

#[test]
fn compose_source_summary_has_required_front_matter() {
    let input = sample_summary_input(SummaryTreeKind::Source, "gmail:alice@x.com|bob@y.com", 1);
    let composed = compose_summary_md(&input);
    let fm = &composed.front_matter;
    assert!(fm.starts_with("---\n"));
    assert!(fm.ends_with("---\n"));
    assert!(fm.contains("kind: summary"));
    assert!(fm.contains("tree_kind: source"));
    assert!(fm.contains("level: 1"));
    assert!(fm.contains("child_count: 2"));
    assert!(fm.contains(&format!(
        "openhuman_core_version: {}",
        OPENHUMAN_CORE_VERSION
    )));
    assert!(fm.contains(&format!(
        "memory_artifact_format: {}",
        MEMORY_ARTIFACT_FORMAT
    )));
    assert!(fm.contains("  - \"[[child-1]]\""));
    assert!(fm.contains("  - \"[[child-2]]\""));
    assert!(fm.contains("  - source/"));
    assert!(fm.contains("aliases:"));
    assert!(composed.body == "This is the summariser output.\n");
    assert!(composed.full.ends_with("This is the summariser output.\n"));
}

#[test]
fn children_are_emitted_as_obsidian_wikilinks() {
    let input = sample_summary_input(SummaryTreeKind::Source, "gmail:alice@x.com", 1);
    let composed = compose_summary_md(&input);
    let fm = &composed.front_matter;
    for id in ["child-1", "child-2"] {
        let expected = format!("  - \"[[{id}]]\"");
        assert!(fm.contains(&expected), "got:\n{fm}");
        let plain = format!("  - {id}\n");
        assert!(!fm.contains(&plain), "got:\n{fm}");
    }
}

#[test]
fn child_basename_overrides_replace_chunk_id_in_wikilink() {
    let ts = chrono::Utc.timestamp_millis_opt(1_700_000_000_000).unwrap();
    let child_ids = vec!["abc123hash".to_string(), "def456hash".to_string()];
    let overrides: Vec<Option<String>> = vec![Some("1700000000000_msg-id-1".into()), None];
    let input = SummaryComposeInput {
        summary_id: "summary:L1:test",
        tree_kind: SummaryTreeKind::Source,
        tree_id: "t1",
        tree_scope: "gmail:alice@x.com",
        level: 1,
        child_ids: &child_ids,
        child_basenames: Some(&overrides),
        child_count: 2,
        time_range_start: ts,
        time_range_end: ts,
        sealed_at: ts,
        body: "L1 body",
    };
    let composed = compose_summary_md(&input);
    let fm = &composed.front_matter;
    assert!(
        fm.contains(r#"  - "[[1700000000000_msg-id-1]]""#),
        "got:\n{fm}"
    );
    assert!(fm.contains(r#"  - "[[def456hash]]""#), "got:\n{fm}");
}

#[test]
fn structured_child_summary_id_is_sanitised_in_wikilink() {
    let ts = chrono::Utc.timestamp_millis_opt(1_700_000_000_000).unwrap();
    let child_id = "summary:L1:b9fa5f08-bf79-41a7-a5c8-2d87883d5c01";
    let expected_basename = "summary-L1-b9fa5f08-bf79-41a7-a5c8-2d87883d5c01";
    let input = SummaryComposeInput {
        summary_id: "summary:L2:cc9a1224",
        tree_kind: SummaryTreeKind::Source,
        tree_id: "t1",
        tree_scope: "gmail:alice@x.com",
        level: 2,
        child_ids: &[child_id.to_string()],
        child_basenames: None,
        child_count: 1,
        time_range_start: ts,
        time_range_end: ts,
        sealed_at: ts,
        body: "L2 body",
    };
    let composed = compose_summary_md(&input);
    let fm = &composed.front_matter;
    let expected = format!("  - \"[[{expected_basename}]]\"");
    assert!(fm.contains(&expected), "got:\n{fm}");
    assert!(!fm.contains(&format!("[[{child_id}]]")), "got:\n{fm}");
}

#[test]
fn compose_global_summary_alias_format() {
    let input = sample_summary_input(SummaryTreeKind::Global, "global", 0);
    let composed = compose_summary_md(&input);
    assert!(composed.front_matter.contains("tree_kind: global"));
    assert!(composed.front_matter.contains("global digest"));
}

#[path = "compose_tests_more.rs"]
mod more;
