use super::*;
use crate::memory::chunks::{Chunk, Metadata, SourceKind};
use crate::memory::store::content::atomic::{sha256_hex, write_if_new};
use crate::memory::store::content::compose::{
    compose_chunk_file, compose_summary_md, rewrite_summary_tags, split_front_matter,
    SummaryComposeInput,
};
use crate::memory::store::content::paths::SummaryTreeKind;
use chrono::TimeZone;
use tempfile::TempDir;

fn sample_chunk() -> Chunk {
    let ts = chrono::Utc.timestamp_millis_opt(1_700_000_000_000).unwrap();
    Chunk {
        id: "tags_test".into(),
        content: "hello from tags test".into(),
        metadata: Metadata {
            source_kind: SourceKind::Chat,
            source_id: "slack:#eng".into(),
            owner: "alice".into(),
            timestamp: ts,
            time_range: (ts, ts),
            tags: vec!["old/Tag".into()],
            source_ref: None,
            path_scope: None,
        },
        token_count: 4,
        seq_in_source: 0,
        created_at: ts,
        partial_message: false,
    }
}

#[test]
fn update_chunk_tags_replaces_tag_block() {
    let dir = TempDir::new().unwrap();
    let chunk = sample_chunk();
    let (full, _) = compose_chunk_file(&chunk);
    let path = dir.path().join("0.md");
    write_if_new(&path, &full).unwrap();

    update_chunk_tags(
        &path,
        &["person/Alice-Smith".into(), "project/Phoenix".into()],
    )
    .unwrap();

    let updated = std::fs::read_to_string(&path).unwrap();
    assert!(updated.contains("  - person/Alice-Smith"));
    assert!(updated.contains("  - project/Phoenix"));
    assert!(!updated.contains("  - old/Tag"));
    assert!(updated.contains("  - source/slack-eng"));
    assert!(updated.ends_with("hello from tags test"));
}

#[test]
fn update_chunk_tags_prefers_path_scope_for_source_tag() {
    let dir = TempDir::new().unwrap();
    let mut chunk = sample_chunk();
    chunk.metadata.source_id = "notion:conn-1:page-123".into();
    chunk.metadata.path_scope = Some("notion:conn-1".into());
    let (full, _) = compose_chunk_file(&chunk);
    let path = dir.path().join("0.md");
    write_if_new(&path, &full).unwrap();

    update_chunk_tags(&path, &["project/Phoenix".into()]).unwrap();

    let updated = std::fs::read_to_string(&path).unwrap();
    assert!(updated.contains("path_scope: \"notion:conn-1\""));
    assert!(updated.contains("  - source/notion-conn-1"));
    assert!(!updated.contains("  - source/notion-conn-1-page-123"));
    assert!(updated.contains("  - project/Phoenix"));
}

#[test]
fn compose_chunk_file_seeds_source_tag() {
    let chunk = sample_chunk();
    let (full, _) = compose_chunk_file(&chunk);
    let text = std::str::from_utf8(&full).unwrap();
    assert!(text.contains("  - source/slack-eng"), "{text}");
    assert!(text.contains("  - old/Tag"), "{text}");
}

#[test]
fn update_chunk_tags_is_noop_for_missing_file() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("nonexistent.md");
    assert!(update_chunk_tags(&path, &["p/X".into()]).is_ok());
}

#[test]
fn slugify_tag_kind_examples() {
    assert_eq!(slugify_tag_kind("Person"), "person");
    assert_eq!(slugify_tag_kind("GitHub Repo"), "github-repo");
    assert_eq!(slugify_tag_kind("EMAIL"), "email");
}

#[test]
fn slugify_tag_value_capitalises_words() {
    assert_eq!(slugify_tag_value("alice johnson"), "Alice-Johnson");
    assert_eq!(slugify_tag_value("project Phoenix"), "Project-Phoenix");
    assert_eq!(slugify_tag_value("OPENAI"), "OPENAI");
}

#[test]
fn entity_tag_builds_obsidian_tag() {
    assert_eq!(
        entity_tag("person", "Alice Johnson"),
        "person/Alice-Johnson"
    );
    assert_eq!(entity_tag("ORG", "Tinyhumans AI"), "org/Tinyhumans-AI");
}

#[test]
fn rewrite_summary_tags_preserves_body_and_replaces_tags() {
    let dir = TempDir::new().unwrap();
    let ts = chrono::Utc.timestamp_millis_opt(1_700_000_000_000).unwrap();
    let body = "summary body for tag test\n";
    let children = vec!["c1".to_string()];
    let input = SummaryComposeInput {
        summary_id: "sum:L1:tagtest",
        tree_kind: SummaryTreeKind::Source,
        tree_id: "t1",
        tree_scope: "gmail:alice@x.com",
        level: 1,
        child_ids: &children,
        child_basenames: None,
        child_count: 1,
        time_range_start: ts,
        time_range_end: ts,
        sealed_at: ts,
        body,
    };
    let composed = compose_summary_md(&input);
    let path = dir.path().join("sum.md");
    write_if_new(&path, composed.full.as_bytes()).unwrap();

    let original = std::fs::read_to_string(&path).unwrap();
    assert!(original.contains("  - source/"), "{original}");

    let new_tags = vec!["person/Alice-Smith".to_string(), "topic/Memory".to_string()];
    let file_bytes = std::fs::read(&path).unwrap();
    let rewritten = rewrite_summary_tags(&file_bytes, &new_tags).unwrap();

    let tmp = dir.path().join("sum.tmp.md");
    {
        use std::io::Write;
        let mut f = std::fs::File::create(&tmp).unwrap();
        f.write_all(&rewritten).unwrap();
    }
    std::fs::rename(&tmp, &path).unwrap();

    let updated = std::fs::read_to_string(&path).unwrap();
    assert!(updated.contains("  - person/Alice-Smith"));
    assert!(updated.contains("  - topic/Memory"));
    assert!(!updated.contains("tags: []"));
    assert!(updated.ends_with(body));

    let (_, body_after) = split_front_matter(&updated).unwrap();
    let sha = sha256_hex(body_after.as_bytes());
    let expected_sha = sha256_hex(body.as_bytes());
    assert_eq!(
        sha, expected_sha,
        "body sha must be stable after tag rewrite"
    );
}
