use super::*;
use crate::memory::store::content::raw::RawKind;

#[test]
fn git_log_args_default_to_head_without_branch() {
    // With no branch configured the walk must stay on the bare clone's HEAD
    // (the default branch), matching the REST fallback's default-branch scope
    // rather than walking every ref.
    let args = git::log_args(50, None, &[]);
    assert_eq!(
        args,
        vec![
            "log".to_string(),
            "HEAD".to_string(),
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
    // Empty/whitespace branch falls back to HEAD, never an empty ref.
    let args = git::log_args(1, Some(""), &[]);
    assert_eq!(args[1], "HEAD");
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
fn commit_list_queries_percent_encode_special_chars() {
    // `&`, `#`, `=` and spaces inside a branch or path value would be parsed
    // as query syntax and corrupt the filter; they must be percent-encoded.
    // `/` is left intact (legal in a query component, and GitHub's commits
    // `path` filter expects the common `path=src/` shape unencoded).
    assert_eq!(
        api::commit_list_queries(Some("feature/one&two"), &["src/#1.rs".to_string()]),
        vec![String::from("sha=feature/one%26two&path=src/%231.rs")]
    );
    // Unreserved values are unchanged.
    assert_eq!(
        api::commit_list_queries(Some("main"), &["docs/".to_string()]),
        vec![String::from("sha=main&path=docs/")]
    );
}

#[tokio::test]
async fn fetch_all_pages_keeps_page_size_constant_and_truncates() {
    // Regression: the page size must not shrink mid-walk. With `max = 150`
    // (not a multiple of 100), a shrinking `per_page` would re-window the
    // offsets — page 2 at per_page=50 returns items 51-100 again, skipping
    // 101-150. A constant page size walks page 1 and page 2 both at
    // per_page=100 and truncates the 200 collected rows to 150.
    let mut requested: Vec<String> = Vec::new();
    let pages = api::collect_pages::<u64, _, _>("commits", 150, |page| {
        let url = format!("per_page=100&page={page}");
        requested.push(url);
        // 100 rows per page, all full (never a short page before the cap).
        let rows: Vec<String> = (1..=100)
            .map(|i| format!("{}", (page - 1) * 100 + i))
            .collect();
        async move { Ok(format!("[{}]", rows.join(","))) }
    })
    .await
    .unwrap();

    assert_eq!(
        requested,
        vec![
            "per_page=100&page=1".to_string(),
            "per_page=100&page=2".to_string(),
        ]
    );
    assert_eq!(pages.len(), 150);
    // No overlap: the second page is the next window (101..), not 51..100.
    assert_eq!(pages[0], 1);
    assert_eq!(pages[100], 101);
    assert_eq!(pages[149], 150);
}

#[tokio::test]
async fn fetch_all_pages_stops_at_a_short_page() {
    // A short page (fewer than GH_PAGE_SIZE rows) is the last page; the walk
    // must not request page 2 after it.
    let mut requested: Vec<u32> = Vec::new();
    let pages = crate::memory::sources::readers::github::api::collect_pages::<u64, _, _>(
        "commits",
        1000,
        |page| {
            requested.push(page);
            async move {
                // Page 1 is short (3 rows) — stop after it even though max is large.
                Ok("[1,2,3]".to_string())
            }
        },
    )
    .await
    .unwrap();

    assert_eq!(requested, vec![1]);
    assert_eq!(pages, vec![1, 2, 3]);
}

/// Build a synthetic `GhCommit` for merge tests.
fn gh_commit(sha: &str, subject: &str, ts: &str) -> types::GhCommit {
    types::GhCommit {
        sha: sha.into(),
        commit: types::GhCommitInner {
            message: subject.into(),
            author: None,
            committer: Some(types::GhAuthor {
                name: None,
                email: None,
                date: Some(ts.into()),
            }),
        },
        author: None,
    }
}

#[test]
fn merge_commit_batches_walks_every_path_before_truncating() {
    // Two configured paths: the first returns two commits, the second one.
    // The pre-fix code stopped after the first path once `out` reached `max`,
    // silently dropping the `src` commit even though it is newer than the
    // second `docs` commit.
    let docs = vec![gh_commit("a", "docs first", "2024-01-01T00:00:00Z")];
    let src = vec![gh_commit("b", "src newer", "2024-02-01T00:00:00Z")];

    let merged = api::merge_commit_batches(vec![docs, src], 3);
    let ids: Vec<&str> = merged.iter().map(|i| i.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["commit:b", "commit:a"],
        "newest-first, both paths kept"
    );
}

#[test]
fn merge_commit_batches_dedups_by_sha_and_truncates_globally() {
    // A commit touching both paths appears in both batches but only once.
    let docs = vec![
        gh_commit("a", "docs first", "2024-01-01T00:00:00Z"),
        gh_commit("shared", "touches both", "2024-02-01T00:00:00Z"),
    ];
    let src = vec![gh_commit("shared", "touches both", "2024-02-01T00:00:00Z")];

    let merged = api::merge_commit_batches(vec![docs, src], 1);
    let ids: Vec<&str> = merged.iter().map(|i| i.id.as_str()).collect();
    assert_eq!(ids, vec!["commit:shared"], "deduped and truncated to max");
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
