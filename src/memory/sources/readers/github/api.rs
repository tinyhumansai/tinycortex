//! `gh` CLI + REST API helpers for the GitHub reader.
//!
//! [`fetch_github`] prefers the authenticated `gh api` path and falls back to
//! the unauthenticated REST API. Commit list/read helpers live here; issue and
//! pull-request list/read helpers live in the sibling `super::issues` module,
//! and commit reads additionally have a local `git` path in the sibling
//! `super::git` module.
//!
//! Branch/path filters are honored on the commits list: `sha=<branch>` and
//! `path=<path>` query params narrow what the API returns to the configured
//! scope.

use std::collections::HashSet;

use crate::memory::sources::types::{ContentType, SourceContent, SourceItem};

use super::types::GhCommit;
use super::{parse_iso_ts, GH_CLI_TIMEOUT};

/// GitHub REST API maximum page size (`per_page`).
pub(super) const GH_PAGE_SIZE: u32 = 100;

/// Hard ceiling on pagination loops so a misbehaving API (always returning a
/// full page) can never spin forever even if `max` is enormous.
pub(super) const GH_MAX_PAGES: u32 = 1000;

/// Run `gh <args>` and return stdout as UTF-8.
pub(super) async fn gh_json(args: &[&str]) -> Result<String, String> {
    let output = tokio::time::timeout(
        GH_CLI_TIMEOUT,
        tokio::process::Command::new("gh").args(args).output(),
    )
    .await
    .map_err(|_| format!("gh command timed out after {}s", GH_CLI_TIMEOUT.as_secs()))?
    .map_err(|e| format!("gh command failed: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("gh exited {}: {stderr}", output.status));
    }

    String::from_utf8(output.stdout).map_err(|e| format!("gh output not utf8: {e}"))
}

/// Unauthenticated GET against the GitHub REST API.
pub(super) async fn api_get(path: &str) -> Result<String, String> {
    let url = format!("https://api.github.com{path}");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| format!("failed to build GitHub client: {e}"))?;
    let resp = client
        .get(&url)
        .header("User-Agent", "openhuman")
        .header("Accept", "application/vnd.github.v3+json")
        .send()
        .await
        .map_err(|e| format!("GitHub API request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("GitHub API returned {status}: {body}"));
    }

    resp.text()
        .await
        .map_err(|e| format!("failed to read response: {e}"))
}

/// Try `gh api` first, fall back to unauthenticated REST API.
pub(super) async fn fetch_github(api_path: &str, use_gh: bool) -> Result<String, String> {
    if use_gh {
        match gh_json(&["api", api_path]).await {
            Ok(s) => return Ok(s),
            Err(e) => {
                tracing::debug!(
                    error = %e,
                    path = %api_path,
                    "[memory_sources:github] gh failed, falling back to API"
                );
            }
        }
    }
    api_get(&format!("/{api_path}")).await
}

/// Fetch up to `max` rows from a paginated GitHub list endpoint.
///
/// Walks `?per_page=100&page=N` with a constant page size — GitHub's
/// offset-based pagination is per_page-relative, so shrinking the page size
/// mid-walk would re-window the offsets and silently skip rows (e.g. `max=150`
/// would fetch items 51-100 a second time instead of 101-150). Iteration stops
/// once `max` rows are collected or the API returns a short page (the last
/// page); `extra_query` is appended verbatim (e.g. `"state=all"`). The result
/// is truncated to exactly `max`.
pub(super) async fn fetch_all_pages<T: serde::de::DeserializeOwned>(
    owner: &str,
    repo: &str,
    resource: &str,
    extra_query: &str,
    max: u32,
    use_gh: bool,
) -> Result<Vec<T>, String> {
    let fetch = |page: u32| async_fetch_page(page, owner, repo, resource, extra_query, use_gh);
    collect_pages(resource, max, fetch).await
}

/// Fetch one page's raw JSON at a constant [`GH_PAGE_SIZE`].
async fn async_fetch_page(
    page: u32,
    owner: &str,
    repo: &str,
    resource: &str,
    extra_query: &str,
    use_gh: bool,
) -> Result<String, String> {
    let mut path = format!("repos/{owner}/{repo}/{resource}?per_page={GH_PAGE_SIZE}&page={page}");
    if !extra_query.is_empty() {
        path.push('&');
        path.push_str(extra_query);
    }
    fetch_github(&path, use_gh).await
}

/// Core pagination walk, split out from [`fetch_all_pages`] so the loop is
/// unit-testable with a fake fetch instead of a live GitHub API.
///
/// `fetch` maps a 1-based page number to the raw JSON for that page. The page
/// size the fetch encodes must stay constant across pages — see
/// [`fetch_all_pages`] for why shrinking it mid-walk skips rows.
pub(super) async fn collect_pages<T, F, Fut>(
    label: &str,
    max: u32,
    mut fetch: F,
) -> Result<Vec<T>, String>
where
    T: serde::de::DeserializeOwned,
    F: FnMut(u32) -> Fut,
    Fut: std::future::Future<Output = Result<String, String>>,
{
    let mut out: Vec<T> = Vec::new();
    let mut page = 1u32;

    while (out.len() as u32) < max && page <= GH_MAX_PAGES {
        let json_str = fetch(page).await?;
        let batch: Vec<T> = serde_json::from_str(&json_str)
            .map_err(|e| format!("parse {label} page {page}: {e}"))?;
        let got = batch.len();
        out.extend(batch);

        // Short page ⇒ no more rows upstream.
        if got < GH_PAGE_SIZE as usize {
            break;
        }
        page += 1;
    }

    out.truncate(max as usize);
    Ok(out)
}

/// Percent-encode a branch or path value for use as a URL query parameter.
///
/// RFC 3986 unreserved characters and `/` are kept as-is; everything else
/// (`&`, `=`, `#`, `?`, `%`, spaces, …) is percent-encoded so a value cannot
/// be misparsed as query syntax and corrupt the filter. `/` is left intact
/// because it is legal in a query component and GitHub's commits `sha`/`path`
/// filters expect the common `path=src/lib.rs` shape unencoded.
fn percent_encode_query(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Build the `extra_query` strings for the commits endpoint — one per
/// configured path (the endpoint accepts a single `path` filter), each
/// carrying the branch's `sha` when set. An empty path list means "no path
/// filter" (a single query carrying only the branch filter, if any).
/// Branch/path values are percent-encoded so `&`, `#`, `=` inside them cannot
/// corrupt the query. Extracted as a pure helper so the filter wiring is
/// unit-testable.
pub(super) fn commit_list_queries(branch: Option<&str>, paths: &[String]) -> Vec<String> {
    let sha_q = branch
        .filter(|b| !b.is_empty())
        .map(|b| format!("sha={}", percent_encode_query(b)));
    let path_qs: Vec<String> = if paths.is_empty() {
        vec![String::new()]
    } else {
        paths
            .iter()
            .map(|p| format!("path={}", percent_encode_query(p)))
            .collect()
    };
    path_qs
        .into_iter()
        .map(|path_q| {
            let mut extra = String::new();
            if let Some(q) = &sha_q {
                extra.push_str(q);
            }
            if !path_q.is_empty() {
                if !extra.is_empty() {
                    extra.push('&');
                }
                extra.push_str(&path_q);
            }
            extra
        })
        .collect()
}

/// List commits via the REST `commits` endpoint (fallback when local git is
/// unavailable).
///
/// A configured `branch` is sent as `sha=<branch>`. The GitHub commits
/// endpoint accepts a single `path` filter, so multiple configured paths are
/// fetched one query each (each bounded at `max` so the walk stays finite),
/// merged and deduped by sha, ordered by commit time, and truncated to `max`.
pub(super) async fn list_commits_api(
    owner: &str,
    repo: &str,
    max: u32,
    use_gh: bool,
    branch: Option<&str>,
    paths: &[String],
) -> Result<Vec<SourceItem>, String> {
    let mut batches: Vec<Vec<GhCommit>> = Vec::new();
    for extra in commit_list_queries(branch, paths) {
        let commits: Vec<GhCommit> =
            fetch_all_pages(owner, repo, "commits", &extra, max, use_gh).await?;
        batches.push(commits);
    }
    Ok(merge_commit_batches(batches, max))
}

/// Merge per-path commit batches into the final item list.
///
/// The GitHub commits endpoint accepts a single `path` filter, so multiple
/// configured paths are fetched one query each; every path must be walked
/// (not just until the first fills `max`) or later paths are silently starved.
/// Batches are deduped by sha, ordered newest-first by commit time, and
/// truncated to `max` — the same union semantics the local `git log` path gives
/// a multi-pathspec walk. Extracted as a pure helper so the merge is
/// unit-testable without a live API.
pub(super) fn merge_commit_batches(batches: Vec<Vec<GhCommit>>, max: u32) -> Vec<SourceItem> {
    let mut out: Vec<SourceItem> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for commits in batches {
        for c in commits {
            if seen.insert(c.sha.clone()) {
                let title = c.commit.message.lines().next().unwrap_or("").to_string();
                let ts = c
                    .commit
                    .committer
                    .as_ref()
                    .and_then(|a| a.date.as_deref())
                    .and_then(parse_iso_ts);
                out.push(SourceItem {
                    id: format!("commit:{}", c.sha),
                    title,
                    updated_at_ms: ts,
                });
            }
        }
    }
    // Each path's query returns its commits newest-first, but the merged set
    // is path-ordered. Re-sort by commit time (newest first) so the global
    // truncation keeps the most recent commits across all configured paths.
    out.sort_by_key(|b| std::cmp::Reverse(b.updated_at_ms));
    out.truncate(max as usize);
    out
}

/// Read one commit via the REST API (fallback when local git is unavailable).
pub(super) async fn read_commit_api(
    owner: &str,
    repo: &str,
    sha: &str,
    use_gh: bool,
) -> Result<SourceContent, String> {
    let json_str = fetch_github(&format!("repos/{owner}/{repo}/commits/{sha}"), use_gh).await?;

    let commit: GhCommit =
        serde_json::from_str(&json_str).map_err(|e| format!("parse commit: {e}"))?;

    let author = commit
        .commit
        .author
        .as_ref()
        .map(|a| {
            format!(
                "{} <{}>",
                a.name.as_deref().unwrap_or("unknown"),
                a.email.as_deref().unwrap_or("")
            )
        })
        .unwrap_or_default();

    // GitHub login of the committer, rendered as an `@handle` so the
    // entity extractor registers it as a `handle:` entity in the memory
    // tree (unique committers become first-class entities).
    let handle = commit
        .author
        .as_ref()
        .map(|u| format!("@{}", u.login))
        .unwrap_or_default();

    let date = commit
        .commit
        .committer
        .as_ref()
        .and_then(|a| a.date.as_deref())
        .unwrap_or("unknown");

    let title = commit
        .commit
        .message
        .lines()
        .next()
        .unwrap_or("")
        .to_string();

    let author_line = if handle.is_empty() {
        author.clone()
    } else {
        format!("{author} ({handle})")
    };

    let body = format!(
        "# Commit: {title}\n\n\
         **SHA:** {sha}\n\
         **Author:** {author_line}\n\
         **Date:** {date}\n\n\
         ## Message\n\n\
         {}",
        commit.commit.message,
    );

    Ok(SourceContent {
        id: format!("commit:{sha}"),
        title,
        body,
        content_type: ContentType::Markdown,
        metadata: serde_json::json!({
            "owner": owner,
            "repo": repo,
            "sha": sha,
            "author": author,
            "author_handle": commit.author.as_ref().map(|u| u.login.clone()),
        }),
    })
}
