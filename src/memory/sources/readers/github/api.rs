//! `gh` CLI + REST API helpers for the GitHub reader.
//!
//! [`fetch_github`] prefers the authenticated `gh api` path and falls back to
//! the unauthenticated REST API. List and read endpoints for commits, issues,
//! and PRs live here; commit reads additionally have a local `git` path in the
//! sibling [`super::git`] module.
//!
//! Branch/path filters are honored on the commits list: `sha=<branch>` and
//! `path=<path>` query params narrow what the API returns to the configured
//! scope.

use std::collections::HashSet;

use serde::Deserialize;

use crate::memory::sources::types::{ContentType, SourceContent, SourceItem};

use super::types::{CachedItem, GhCommit, GhIssue, GhPr, GhUser};
use super::{parse_iso_ts, unique_handles, GH_CLI_TIMEOUT, LIST_CACHE};

/// GitHub REST API maximum page size (`per_page`).
const GH_PAGE_SIZE: u32 = 100;

/// Hard ceiling on pagination loops so a misbehaving API (always returning a
/// full page) can never spin forever even if `max` is enormous.
const GH_MAX_PAGES: u32 = 1000;

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
/// Walks `?per_page=100&page=N` until `max` rows are collected or the API
/// returns a short page (the last page). `extra_query` is appended verbatim
/// (e.g. `"state=all"`). The result is truncated to exactly `max`.
async fn fetch_all_pages<T: serde::de::DeserializeOwned>(
    owner: &str,
    repo: &str,
    resource: &str,
    extra_query: &str,
    max: u32,
    use_gh: bool,
) -> Result<Vec<T>, String> {
    let mut out: Vec<T> = Vec::new();
    let mut page = 1u32;

    while (out.len() as u32) < max && page <= GH_MAX_PAGES {
        let remaining = max - out.len() as u32;
        let per_page = remaining.min(GH_PAGE_SIZE);
        let mut path = format!("repos/{owner}/{repo}/{resource}?per_page={per_page}&page={page}");
        if !extra_query.is_empty() {
            path.push('&');
            path.push_str(extra_query);
        }

        let json_str = fetch_github(&path, use_gh).await?;
        let batch: Vec<T> = serde_json::from_str(&json_str)
            .map_err(|e| format!("parse {resource} page {page}: {e}"))?;
        let got = batch.len();
        out.extend(batch);

        // Short page ⇒ no more rows upstream.
        if got < per_page as usize {
            break;
        }
        page += 1;
    }

    out.truncate(max as usize);
    Ok(out)
}

/// Build the `extra_query` strings for the commits endpoint — one per
/// configured path (the endpoint accepts a single `path` filter), each
/// carrying the branch's `sha` when set. An empty path list means "no path
/// filter" (a single query carrying only the branch filter, if any).
/// Extracted as a pure helper so the filter wiring is unit-testable.
pub(super) fn commit_list_queries(branch: Option<&str>, paths: &[String]) -> Vec<String> {
    let sha_q = branch.filter(|b| !b.is_empty()).map(|b| format!("sha={b}"));
    let path_qs: Vec<String> = if paths.is_empty() {
        vec![String::new()]
    } else {
        paths.iter().map(|p| format!("path={p}")).collect()
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
/// fetched one query each and merged, deduped by sha, truncated to `max`.
pub(super) async fn list_commits_api(
    owner: &str,
    repo: &str,
    max: u32,
    use_gh: bool,
    branch: Option<&str>,
    paths: &[String],
) -> Result<Vec<SourceItem>, String> {
    let mut out: Vec<SourceItem> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for extra in commit_list_queries(branch, paths) {
        let commits: Vec<GhCommit> =
            fetch_all_pages(owner, repo, "commits", &extra, max, use_gh).await?;
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
                if out.len() as u32 >= max {
                    return Ok(out);
                }
            }
        }
        if out.len() as u32 >= max {
            break;
        }
    }
    Ok(out)
}

/// List issues (excluding pull requests, which the issues endpoint also
/// returns) with the full row cached for later reads.
pub(super) async fn list_issues(
    owner: &str,
    repo: &str,
    max: u32,
    use_gh: bool,
) -> Result<Vec<SourceItem>, String> {
    let mut out: Vec<SourceItem> = Vec::new();
    let mut page = 1u32;

    while (out.len() as u32) < max && page <= GH_MAX_PAGES {
        let path =
            format!("repos/{owner}/{repo}/issues?per_page={GH_PAGE_SIZE}&page={page}&state=all");
        let json_str = fetch_github(&path, use_gh).await?;
        let batch: Vec<GhIssue> = serde_json::from_str(&json_str)
            .map_err(|e| format!("parse issues page {page}: {e}"))?;
        let got = batch.len();

        for i in batch {
            if i.pull_request.is_some() {
                continue;
            }
            let ts = i.updated_at.as_deref().and_then(parse_iso_ts);
            let item_id = format!("issue:{}", i.number);
            let cache_key = format!("{owner}/{repo}:{item_id}");
            out.push(SourceItem {
                id: item_id,
                title: format!("#{} {}", i.number, i.title),
                updated_at_ms: ts,
            });
            if let Ok(mut cache) = LIST_CACHE.lock() {
                cache.insert(cache_key, CachedItem::Issue(i));
            }
            if out.len() as u32 >= max {
                break;
            }
        }

        if got < GH_PAGE_SIZE as usize {
            break;
        }
        page += 1;
    }

    Ok(out)
}

/// List pull requests with the full row cached for later reads.
pub(super) async fn list_prs(
    owner: &str,
    repo: &str,
    max: u32,
    use_gh: bool,
) -> Result<Vec<SourceItem>, String> {
    let prs: Vec<GhPr> = fetch_all_pages(owner, repo, "pulls", "state=all", max, use_gh).await?;

    let items: Vec<SourceItem> = prs
        .into_iter()
        .map(|p| {
            let ts = p.updated_at.as_deref().and_then(parse_iso_ts);
            let item_id = format!("pr:{}", p.number);
            let cache_key = format!("{owner}/{repo}:{item_id}");
            let item = SourceItem {
                id: item_id,
                title: format!("PR #{} {}", p.number, p.title),
                updated_at_ms: ts,
            };
            if let Ok(mut cache) = LIST_CACHE.lock() {
                cache.insert(cache_key, CachedItem::Pr(p));
            }
            item
        })
        .collect();

    Ok(items)
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

/// Read one issue, preferring the row cached by the list pass.
pub(super) async fn read_issue(
    owner: &str,
    repo: &str,
    number: u64,
    use_gh: bool,
) -> Result<SourceContent, String> {
    let cache_key = format!("{owner}/{repo}:issue:{number}");
    let from_cache = LIST_CACHE
        .lock()
        .ok()
        .and_then(|mut c| c.remove(&cache_key));
    let issue: GhIssue = match from_cache {
        Some(CachedItem::Issue(i)) => i,
        _ => {
            let json_str =
                fetch_github(&format!("repos/{owner}/{repo}/issues/{number}"), use_gh).await?;
            serde_json::from_str(&json_str).map_err(|e| format!("parse issue: {e}"))?
        }
    };

    let author = issue
        .user
        .as_ref()
        .map(|u| u.login.as_str())
        .unwrap_or("unknown");
    let labels: Vec<&str> = issue.labels.iter().map(|l| l.name.as_str()).collect();
    let issue_body = issue.body.as_deref().unwrap_or("");

    let comments = fetch_issue_comments(owner, repo, number, use_gh).await;
    let participants =
        unique_handles(std::iter::once(author).chain(comments.iter().map(|c| c.user.as_str())));

    let mut body = format!(
        "# Issue #{number}: {title}\n\n\
         **State:** {state}\n\
         **Author:** @{author}\n\
         **Participants:** {participants}\n\
         **Labels:** {label_str}\n\
         **Created:** {created}\n\
         **Updated:** {updated}\n\n\
         ## Description\n\n\
         {issue_body}",
        title = issue.title,
        state = issue.state,
        label_str = if labels.is_empty() {
            "none".to_string()
        } else {
            labels.join(", ")
        },
        created = issue.created_at.as_deref().unwrap_or("unknown"),
        updated = issue.updated_at.as_deref().unwrap_or("unknown"),
    );

    if !comments.is_empty() {
        body.push_str("\n\n## Comments\n");
        for comment in &comments {
            body.push_str(&format!(
                "\n### @{} ({})\n\n{}\n",
                comment.user, comment.created_at, comment.body
            ));
        }
    }

    Ok(SourceContent {
        id: format!("issue:{number}"),
        title: format!("#{number} {}", issue.title),
        body,
        content_type: ContentType::Markdown,
        metadata: serde_json::json!({
            "owner": owner,
            "repo": repo,
            "number": number,
            "state": issue.state,
            "labels": labels,
        }),
    })
}

/// Read one pull request, preferring the row cached by the list pass.
pub(super) async fn read_pr(
    owner: &str,
    repo: &str,
    number: u64,
    use_gh: bool,
) -> Result<SourceContent, String> {
    let cache_key = format!("{owner}/{repo}:pr:{number}");
    let from_cache = LIST_CACHE
        .lock()
        .ok()
        .and_then(|mut c| c.remove(&cache_key));
    let pr: GhPr = match from_cache {
        Some(CachedItem::Pr(p)) => p,
        _ => {
            let json_str =
                fetch_github(&format!("repos/{owner}/{repo}/pulls/{number}"), use_gh).await?;
            serde_json::from_str(&json_str).map_err(|e| format!("parse PR: {e}"))?
        }
    };

    let author = pr
        .user
        .as_ref()
        .map(|u| u.login.as_str())
        .unwrap_or("unknown");
    let labels: Vec<&str> = pr.labels.iter().map(|l| l.name.as_str()).collect();
    let pr_body = pr.body.as_deref().unwrap_or("");

    let merged_str = match pr.merged_at.as_deref() {
        Some(ts) => format!("merged at {ts}"),
        None => "not merged".to_string(),
    };

    let comments = fetch_issue_comments(owner, repo, number, use_gh).await;
    let participants =
        unique_handles(std::iter::once(author).chain(comments.iter().map(|c| c.user.as_str())));

    let mut body = format!(
        "# PR #{number}: {title}\n\n\
         **State:** {state} ({merged})\n\
         **Author:** @{author}\n\
         **Participants:** {participants}\n\
         **Labels:** {label_str}\n\
         **Created:** {created}\n\
         **Updated:** {updated}\n\n\
         ## Description\n\n\
         {pr_body}",
        title = pr.title,
        state = pr.state,
        merged = merged_str,
        label_str = if labels.is_empty() {
            "none".to_string()
        } else {
            labels.join(", ")
        },
        created = pr.created_at.as_deref().unwrap_or("unknown"),
        updated = pr.updated_at.as_deref().unwrap_or("unknown"),
    );

    if !comments.is_empty() {
        body.push_str("\n\n## Comments\n");
        for comment in &comments {
            body.push_str(&format!(
                "\n### @{} ({})\n\n{}\n",
                comment.user, comment.created_at, comment.body
            ));
        }
    }

    Ok(SourceContent {
        id: format!("pr:{number}"),
        title: format!("PR #{number} {}", pr.title),
        body,
        content_type: ContentType::Markdown,
        metadata: serde_json::json!({
            "owner": owner,
            "repo": repo,
            "number": number,
            "state": pr.state,
            "merged": pr.merged_at.is_some(),
            "labels": labels,
        }),
    })
}

/// Fetch up to 50 comments on an issue/PR. Best-effort: any failure (or
/// parse error) yields an empty list — comment text is enrichment, not the
/// item's substance, so a missing comments API must not fail the read.
async fn fetch_issue_comments(
    owner: &str,
    repo: &str,
    number: u64,
    use_gh: bool,
) -> Vec<IssueComment> {
    #[derive(Deserialize)]
    struct RawComment {
        user: Option<GhUser>,
        body: Option<String>,
        created_at: Option<String>,
    }

    let json_str = fetch_github(
        &format!("repos/{owner}/{repo}/issues/{number}/comments?per_page=50"),
        use_gh,
    )
    .await;

    let Ok(json_str) = json_str else {
        return Vec::new();
    };

    let comments: Vec<RawComment> = serde_json::from_str(&json_str).unwrap_or_default();

    comments
        .into_iter()
        .map(|c| IssueComment {
            user: c
                .user
                .as_ref()
                .map(|u| u.login.clone())
                .unwrap_or_else(|| "unknown".into()),
            body: c.body.unwrap_or_default(),
            created_at: c.created_at.unwrap_or_else(|| "unknown".into()),
        })
        .collect()
}

struct IssueComment {
    user: String,
    body: String,
    created_at: String,
}
