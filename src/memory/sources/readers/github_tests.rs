use super::*;
use crate::memory::store::content::raw::RawKind;

#[test]
fn git_log_args_default_to_all_refs_without_branch() {
    let args = git::log_args(50, None, &[]);
    assert_eq!(
        args,
        vec![
            "log".to_string(),
            "--all".to_string(),
            "--max-count=50".to_string(),
            "--format=%H\t%s\t%aI".to_string(),
        ]
    );
}

#[test]
fn git_log_args_restrict_to_branch_and_paths() {
    let args = git::log_args(
        50,
        Some("main"),
        &["src/lib.rs".to_string(), "docs/".to_string()],
    );
    assert_eq!(
        args,
        vec![
            "log".to_string(),
            "main".to_string(),
            "--max-count=50".to_string(),
            "--format=%H\t%s\t%aI".to_string(),
            "--".to_string(),
            "src/lib.rs".to_string(),
            "docs/".to_string(),
        ]
    );
    // Empty/whitespace branch falls back to --all, never an empty ref.
    let args = git::log_args(1, Some(""), &[]);
    assert_eq!(args[1], "--all");
}

#[test]
fn commit_list_queries_carry_branch_and_path_filters() {
    // No filters → a single empty query (plain pagination).
    assert_eq!(api::commit_list_queries(None, &[]), vec![String::new()]);
    // Branch only → `sha=<branch>`.
    assert_eq!(
        api::commit_list_queries(Some("main"), &[]),
        vec![String::from("sha=main")]
    );
    // One path → `path=<p>`.
    assert_eq!(
        api::commit_list_queries(None, &["src/".to_string()]),
        vec![String::from("path=src/")]
    );
    // Branch + one path → `sha=<branch>&path=<p>`.
    assert_eq!(
        api::commit_list_queries(Some("main"), &["src/lib.rs".to_string()]),
        vec![String::from("sha=main&path=src/lib.rs")]
    );
    // Multiple paths → one query per path, dedup happens in the caller.
    assert_eq!(
        api::commit_list_queries(Some("main"), &["a/".to_string(), "b/".to_string()]),
        vec![
            String::from("sha=main&path=a/"),
            String::from("sha=main&path=b/")
        ]
    );
    // Empty branch is treated as unset.
    assert_eq!(
        api::commit_list_queries(Some(""), &["a/".to_string()]),
        vec![String::from("path=a/")]
    );
}

#[test]
fn parse_github_url_extracts_owner_and_repo() {
    let (owner, repo) = parse_github_url("https://github.com/openai/tiktoken").unwrap();
    assert_eq!(owner, "openai");
    assert_eq!(repo, "tiktoken");
}

#[test]
fn parse_github_url_handles_trailing_slash_and_git() {
    let (owner, repo) = parse_github_url("https://github.com/org/repo.git/").unwrap();
    assert_eq!(owner, "org");
    assert_eq!(repo, "repo");
}

#[test]
fn parse_github_url_rejects_non_repo_paths() {
    // Deep links like /tree/main must not silently extract the wrong
    // owner/repo. Bare host or non-github URLs also rejected.
    assert!(parse_github_url("https://github.com/org/repo/tree/main").is_err());
    assert!(parse_github_url("https://gitlab.com/org/repo").is_err());
    assert!(parse_github_url("https://github.com/org").is_err());
    assert!(parse_github_url("not-a-url").is_err());
}

#[test]
fn item_kind_round_trips() {
    let cases = [
        ("commit:abc123", ItemKind::Commit, "abc123"),
        ("issue:42", ItemKind::Issue, "42"),
        ("pr:99", ItemKind::PullRequest, "99"),
    ];
    for (id, expected_kind, expected_ref) in cases {
        let (kind, ref_id) = ItemKind::from_id(id).unwrap();
        assert_eq!(kind, expected_kind);
        assert_eq!(ref_id, expected_ref);
    }
}

#[test]
fn item_kind_rejects_invalid() {
    assert!(ItemKind::from_id("unknown:123").is_none());
    assert!(ItemKind::from_id("noprefix").is_none());
}

#[test]
fn repo_archive_source_id_slugs_to_repo_folder() {
    // `github.com/<owner>/<repo>` → slugify → `github-com-<owner>-<repo>`.
    assert_eq!(
        repo_archive_source_id("https://github.com/tinyhumansai/openhuman").as_deref(),
        Some("github.com/tinyhumansai/openhuman")
    );
    assert!(repo_archive_source_id("not-a-url").is_none());
}

#[test]
fn chunk_source_id_is_clean_and_per_item() {
    assert_eq!(
        chunk_source_id("https://github.com/org/repo", "commit:abc123").as_deref(),
        Some("github:org/repo:commit:abc123")
    );
    assert_eq!(
        chunk_source_id("https://github.com/org/repo", "pr:42").as_deref(),
        Some("github:org/repo:pr:42")
    );
}

#[test]
fn unique_handles_dedups_and_skips_unknown() {
    assert_eq!(
        unique_handles(["alice", "bob", "alice", "unknown", ""].into_iter()),
        "@alice @bob"
    );
    assert_eq!(unique_handles(["unknown", ""].into_iter()), "none");
    assert_eq!(unique_handles(std::iter::empty()), "none");
}

#[test]
fn raw_archive_coords_maps_kind_and_uid() {
    assert_eq!(
        raw_archive_coords("commit:deadbeef"),
        Some((RawKind::Commit, "deadbeef".to_string()))
    );
    assert_eq!(
        raw_archive_coords("issue:7"),
        Some((RawKind::Issue, "7".to_string()))
    );
    assert_eq!(
        raw_archive_coords("pr:99"),
        Some((RawKind::PullRequest, "99".to_string()))
    );
    assert!(raw_archive_coords("bogus:1").is_none());
}
