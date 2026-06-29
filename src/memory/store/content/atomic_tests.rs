use super::*;
use crate::memory::store::content::compose::SummaryComposeInput;
use crate::memory::store::content::paths::SummaryTreeKind;
use tempfile::TempDir;

#[test]
fn write_creates_file_and_returns_true() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("sub").join("0.md");
    let written = write_if_new(&path, b"hello world").unwrap();
    assert!(written, "first write must return true");
    assert_eq!(std::fs::read(&path).unwrap(), b"hello world");
}

#[test]
fn write_is_idempotent_returns_false_on_second_call() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("0.md");
    write_if_new(&path, b"first").unwrap();
    let written = write_if_new(&path, b"second").unwrap();
    assert!(!written, "second write must return false");
    assert_eq!(std::fs::read(&path).unwrap(), b"first");
}

#[test]
fn sha256_hex_is_stable() {
    let a = sha256_hex(b"hello");
    let b = sha256_hex(b"hello");
    assert_eq!(a, b);
    assert_ne!(sha256_hex(b"hello"), sha256_hex(b"world"));
    assert_eq!(a.len(), 64);
}

fn mk_summary_input<'a>(
    tree_kind: SummaryTreeKind,
    scope: &'a str,
    id: &'a str,
    body: &'a str,
    children: &'a [String],
) -> SummaryComposeInput<'a> {
    use chrono::TimeZone;
    let ts = chrono::Utc.timestamp_millis_opt(1_700_000_000_000).unwrap();
    SummaryComposeInput {
        summary_id: id,
        tree_kind,
        tree_id: "tree-001",
        tree_scope: scope,
        level: 1,
        child_ids: children,
        child_basenames: None,
        child_count: children.len(),
        time_range_start: ts,
        time_range_end: ts,
        sealed_at: ts,
        body,
    }
}

#[test]
fn stage_summary_writes_file_and_returns_staged() {
    let dir = TempDir::new().unwrap();
    let children = vec!["c1".to_string()];
    let input = mk_summary_input(
        SummaryTreeKind::Source,
        "gmail:alice@x.com",
        "summary:L1:test1",
        "summary body",
        &children,
    );
    let staged = stage_summary(dir.path(), &input, "gmail-alice-x-com").unwrap();
    assert_eq!(staged.summary_id, "summary:L1:test1");
    assert!(staged.content_path.starts_with("wiki/summaries/source-"));
    assert!(staged.content_path.ends_with(".md"));
    assert_eq!(staged.content_sha256.len(), 64);

    let mut abs = dir.path().to_path_buf();
    for part in staged.content_path.split('/') {
        abs.push(part);
    }
    assert!(abs.exists(), "staged file must exist");
}

#[test]
fn stage_summary_is_idempotent() {
    let dir = TempDir::new().unwrap();
    let children = vec!["c1".to_string()];
    let input = mk_summary_input(
        SummaryTreeKind::Topic,
        "person:alex",
        "summary:L1:idem",
        "idempotent body",
        &children,
    );
    let first = stage_summary(dir.path(), &input, "person-alex").unwrap();
    let second = stage_summary(dir.path(), &input, "person-alex").unwrap();
    assert_eq!(first.content_sha256, second.content_sha256);
    assert_eq!(first.content_path, second.content_path);
}

#[test]
fn stage_summary_global_uses_singleton_folder_no_date() {
    let dir = TempDir::new().unwrap();
    let children = vec![];
    let input = mk_summary_input(
        SummaryTreeKind::Global,
        "global",
        "summary:L0:daily",
        "daily recap",
        &children,
    );
    let staged = stage_summary(dir.path(), &input, "global").unwrap();
    assert_eq!(
        staged.content_path, "wiki/summaries/global/L1/summary-L0-daily.md",
        "got: {}",
        staged.content_path
    );
}

#[test]
fn stage_summary_sha256_is_over_body_only() {
    let dir = TempDir::new().unwrap();
    let children = vec![];
    let body = "the body content";
    let input = mk_summary_input(
        SummaryTreeKind::Source,
        "gmail:x@y.com",
        "summary:L1:sha-test",
        body,
        &children,
    );
    let staged = stage_summary(dir.path(), &input, "gmail-x-y-com").unwrap();
    let expected = sha256_hex(body.as_bytes());
    assert_eq!(staged.content_sha256, expected);
}

#[test]
fn stage_summary_rewrites_stale_on_disk_body() {
    let dir = TempDir::new().unwrap();
    let children = vec!["c1".to_string()];
    let new_body = "fresh body for re-stage test";
    let input = mk_summary_input(
        SummaryTreeKind::Source,
        "gmail:stale@test.com",
        "summary:L1:stale-test",
        new_body,
        &children,
    );

    let first = stage_summary(dir.path(), &input, "gmail-stale-test-com").unwrap();

    let mut abs = dir.path().to_path_buf();
    for part in first.content_path.split('/') {
        abs.push(part);
    }
    std::fs::write(&abs, b"---\nstale_key: true\n---\nSTALE BODY CONTENT").unwrap();

    let second = stage_summary(dir.path(), &input, "gmail-stale-test-com").unwrap();

    let expected_sha = sha256_hex(new_body.as_bytes());
    assert_eq!(second.content_sha256, expected_sha);

    let disk_bytes = std::fs::read(&abs).unwrap();
    let disk_str = std::str::from_utf8(&disk_bytes).unwrap();
    assert!(disk_str.contains(new_body));
    assert!(!disk_str.contains("STALE BODY CONTENT"));
}
