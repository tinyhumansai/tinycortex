use super::*;
use tempfile::TempDir;

#[test]
fn slug_account_email_basic() {
    assert_eq!(
        slug_account_email("stevent95@gmail.com"),
        "stevent95-at-gmail-dot-com"
    );
}

#[test]
fn slug_account_email_lowercases_and_trims() {
    assert_eq!(
        slug_account_email("  Alice.Smith@Example.CO.UK "),
        "alice-dot-smith-at-example-dot-co-dot-uk"
    );
}

#[test]
fn slug_account_email_handles_plus_aliases() {
    assert_eq!(
        slug_account_email("alice+work@example.com"),
        "alice-work-at-example-dot-com"
    );
}

#[test]
fn slug_account_email_falls_back_to_unknown() {
    assert_eq!(slug_account_email(""), "unknown");
    assert_eq!(slug_account_email("@@@"), "at-at-at");
    assert_eq!(slug_account_email("///"), "unknown");
}

#[test]
fn write_raw_items_creates_named_files() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let items = [
        RawItem {
            uid: "msg-1",
            created_at_ms: 1_700_000_000_000,
            markdown: "# hello",
            kind: RawKind::Email,
        },
        RawItem {
            uid: "msg-2",
            created_at_ms: 1_700_000_010_000,
            markdown: "# world",
            kind: RawKind::Email,
        },
    ];
    let n = write_raw_items(root, "gmail:stevent95-at-gmail-dot-com", &items).unwrap();
    assert_eq!(n, 2);
    let dir = raw_kind_dir(root, "gmail:stevent95-at-gmail-dot-com", RawKind::Email);
    assert!(dir.exists());
    assert_eq!(
        dir.parent().unwrap(),
        raw_source_dir(root, "gmail:stevent95-at-gmail-dot-com")
    );
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec![
            "1700000000000_msg-1.md".to_string(),
            "1700000010000_msg-2.md".to_string()
        ]
    );
}

#[test]
fn write_raw_items_is_idempotent() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    write_raw_items(
        root,
        "gmail:acct",
        &[RawItem {
            uid: "msg-1",
            created_at_ms: 1_700_000_000_000,
            markdown: "v1",
            kind: RawKind::Email,
        }],
    )
    .unwrap();
    write_raw_items(
        root,
        "gmail:acct",
        &[RawItem {
            uid: "msg-1",
            created_at_ms: 1_700_000_000_000,
            markdown: "v2",
            kind: RawKind::Email,
        }],
    )
    .unwrap();
    let dir = raw_kind_dir(root, "gmail:acct", RawKind::Email);
    let path = dir.join("1700000000000_msg-1.md");
    let body = std::fs::read_to_string(&path).unwrap();
    assert_eq!(body, "v2");
}

#[test]
fn write_raw_items_sanitises_uid_path_chars() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    write_raw_items(
        root,
        "gmail:acct",
        &[RawItem {
            uid: "msg/with:dangerous*chars",
            created_at_ms: 0,
            markdown: "x",
            kind: RawKind::Email,
        }],
    )
    .unwrap();
    let dir = raw_kind_dir(root, "gmail:acct", RawKind::Email);
    let entries: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(entries.len(), 1);
    assert!(entries[0].starts_with("0_msg-with-dangerous-chars"));
}

#[test]
fn write_raw_items_empty_is_noop() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let n = write_raw_items(root, "gmail:acct", &[]).unwrap();
    assert_eq!(n, 0);
    assert!(!raw_source_dir(root, "gmail:acct").exists());
    assert!(!raw_kind_dir(root, "gmail:acct", RawKind::Email).exists());
}

#[test]
fn write_raw_items_splits_kinds_into_subdirs() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let items = [
        RawItem {
            uid: "msg-1",
            created_at_ms: 1_700_000_000_000,
            markdown: "email",
            kind: RawKind::Email,
        },
        RawItem {
            uid: "person-1",
            created_at_ms: 0,
            markdown: "contact",
            kind: RawKind::Contact,
        },
    ];
    let n = write_raw_items(root, "gmail:acct", &items).unwrap();
    assert_eq!(n, 2);
    assert!(raw_kind_dir(root, "gmail:acct", RawKind::Email)
        .join("1700000000000_msg-1.md")
        .exists());
    assert!(raw_kind_dir(root, "gmail:acct", RawKind::Contact)
        .join("0_person-1.md")
        .exists());
}

#[test]
fn raw_rel_path_uses_kind_subdir() {
    assert_eq!(
        raw_rel_path("gmail:acct", RawKind::Email, 1_700_000_000_000, "msg-1"),
        "raw/gmail-acct/emails/1700000000000_msg-1.md"
    );
    assert_eq!(
        raw_rel_path("slack:team", RawKind::Chat, 42, "msg/with:bad"),
        "raw/slack-team/chats/42_msg-with-bad.md"
    );
}

#[test]
fn github_raw_kinds_use_repo_grouped_subdirs() {
    assert_eq!(RawKind::Commit.as_dir(), "commits");
    assert_eq!(RawKind::Issue.as_dir(), "issues");
    assert_eq!(RawKind::PullRequest.as_dir(), "prs");
    assert_eq!(
        raw_rel_path(
            "github.com/tinyhumansai/openhuman",
            RawKind::Commit,
            1_700_000_000_000,
            "2a958e87"
        ),
        "raw/github-com-tinyhumansai-openhuman/commits/1700000000000_2a958e87.md"
    );
    assert_eq!(
        raw_rel_path("github.com/org/repo", RawKind::Issue, 0, "42"),
        "raw/github-com-org-repo/issues/0_42.md"
    );
    assert_eq!(
        raw_rel_path("github.com/org/repo", RawKind::PullRequest, 0, "99"),
        "raw/github-com-org-repo/prs/0_99.md"
    );
}
