//! Issue and pull-request list/read helpers for the GitHub reader.
//!
//! Both endpoints share the [`fetch_github`](super::api::fetch_github)
//! transport and the list-pass cache in `super::types::LIST_CACHE`: the issues
//! endpoint returns pull requests mixed in with issues, and the PR endpoint is
//! the only one that returns merge state, so the list pass stashes the full
//! row and the read pass reuses it instead of re-fetching.

use serde::Deserialize;

use crate::memory::sources::types::{ContentType, SourceContent, SourceItem};

use super::api::{fetch_all_pages, fetch_github, GH_MAX_PAGES, GH_PAGE_SIZE};
use super::types::{CachedItem, GhIssue, GhPr, GhUser, IssueComment};
use super::{parse_iso_ts, unique_handles};

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
            if let Ok(mut cache) = super::types::LIST_CACHE.lock() {
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
            if let Ok(mut cache) = super::types::LIST_CACHE.lock() {
                cache.insert(cache_key, CachedItem::Pr(p));
            }
            item
        })
        .collect();

    Ok(items)
}

/// Read one issue, preferring the row cached by the list pass.
pub(super) async fn read_issue(
    owner: &str,
    repo: &str,
    number: u64,
    use_gh: bool,
) -> Result<SourceContent, String> {
    let cache_key = format!("{owner}/{repo}:issue:{number}");
    let from_cache = super::types::LIST_CACHE
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
    let from_cache = super::types::LIST_CACHE
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
