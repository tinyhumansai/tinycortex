//! Local bare-clone helpers for the GitHub reader.
//!
//! Commits are listed via a per-repo bare clone (`git log`) rather than the
//! REST API whenever the repo is reachable over git: the clone's refs are a
//! superset of what the API exposes and reads are fully offline after the
//! initial clone/fetch. The clone lives under
//! `workspace/git_cache/<owner>/<repo>.git`.
//!
//! Branch/path filters are honored here: a configured `branch` narrows `git
//! log` to that ref (instead of the bare clone's `HEAD`), and configured
//! `paths` become git pathspecs so commits touching unrelated paths are not
//! ingested.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::memory::sources::types::{ContentType, SourceContent, SourceItem};

use super::parse_iso_ts;

/// Timeout for a single `git clone` / `git fetch` (slow on a cold cache).
const GIT_CLONE_TIMEOUT: Duration = Duration::from_secs(120);
/// Timeout for a single `git log` / `git show` (fast, local).
const GIT_LOG_TIMEOUT: Duration = Duration::from_secs(30);

/// Path to the bare clone for a repo, created lazily under
/// `workspace/git_cache/<owner>/<repo>.git`.
pub(super) fn git_cache_dir(workspace: &Path, owner: &str, repo: &str) -> PathBuf {
    workspace
        .join("git_cache")
        .join(owner)
        .join(format!("{repo}.git"))
}

/// Ensure a bare clone of `owner/repo` exists at `cache_dir` — fetching into
/// an existing clone, cloning fresh when absent. A missing remote (private or
/// renamed repo) surfaces as an error handled by the caller's fallback.
pub(super) async fn ensure_bare_clone(
    owner: &str,
    repo: &str,
    cache_dir: &Path,
) -> Result<(), String> {
    if cache_dir.join("HEAD").exists() {
        return fetch_existing_bare(cache_dir).await;
    }

    let clone_url = format!("https://github.com/{owner}/{repo}.git");
    clone_bare(&clone_url, cache_dir).await
}

/// `git fetch` into an existing bare clone.
///
/// The refspec is explicit (`+refs/heads/*:refs/heads/*`): a bare
/// `git clone` records no `remote.origin.fetch` mapping, so a bare `git fetch`
/// without one would only update `FETCH_HEAD` and leave `refs/heads/*` at the
/// initial clone — every later sync would silently miss new GitHub activity.
/// `--prune` also drops local heads the remote has since deleted.
///
/// After the fetch, `HEAD` is refreshed to the remote's current default branch
/// so an unconfigured sync keeps following the repo's default even when that
/// default changes between clones (see [`refresh_default_branch_head`]).
async fn fetch_existing_bare(cache_dir: &Path) -> Result<(), String> {
    tracing::debug!(
        cache = %cache_dir.display(),
        "[memory_sources:github:git] fetching into existing bare clone"
    );
    let output = tokio::time::timeout(
        GIT_CLONE_TIMEOUT,
        tokio::process::Command::new("git")
            .args([
                "fetch",
                "--prune",
                "--quiet",
                "origin",
                "+refs/heads/*:refs/heads/*",
            ])
            .current_dir(cache_dir)
            .output(),
    )
    .await
    .map_err(|_| "git fetch timed out".to_string())?
    .map_err(|e| format!("git fetch failed: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git fetch exited {}: {stderr}", output.status));
    }
    refresh_default_branch_head(cache_dir).await;
    Ok(())
}

/// Repoint the bare clone's `HEAD` to the remote's current default branch.
///
/// `git clone --bare` pins `HEAD` to the default branch selected at clone
/// time, and the fetch refspec above updates `refs/heads/*` but never `HEAD`.
/// If the remote later changes its default branch (while keeping the old
/// branch alive), an unconfigured `git log HEAD` would keep walking the old
/// branch forever, diverging from the REST fallback which follows the new
/// default. Reading the remote `HEAD` symref (`ref: refs/heads/<branch>`) and
/// writing it back keeps the clone's default in sync.
///
/// Best-effort: `git ls-remote` can fail transiently (network), and there is
/// nothing to refresh on an unborn default branch; neither should fail the
/// fetch that already succeeded.
async fn refresh_default_branch_head(cache_dir: &Path) {
    let Ok(output) = tokio::time::timeout(
        GIT_CLONE_TIMEOUT,
        tokio::process::Command::new("git")
            .args(["ls-remote", "--symref", "origin", "HEAD"])
            .current_dir(cache_dir)
            .output(),
    )
    .await
    else {
        return;
    };
    let Ok(output) = output else { return };
    if !output.status.success() {
        return;
    }
    // `--symref` prints `ref: refs/heads/<branch>\tHEAD` on the first line.
    let stdout = String::from_utf8_lossy(&output.stdout);
    let Some(first) = stdout.lines().next() else {
        return;
    };
    let Some(remote_ref) = first
        .strip_prefix("ref: ")
        .and_then(|r| r.split_whitespace().next())
    else {
        return;
    };
    if !remote_ref.starts_with("refs/heads/") {
        return;
    }
    let _ = tokio::time::timeout(
        GIT_CLONE_TIMEOUT,
        tokio::process::Command::new("git")
            .args(["symbolic-ref", "HEAD", remote_ref])
            .current_dir(cache_dir)
            .output(),
    )
    .await;
}

/// Fresh bare clone of `clone_url` into `cache_dir`.
async fn clone_bare(clone_url: &str, cache_dir: &Path) -> Result<(), String> {
    if let Some(parent) = cache_dir.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create cache dir: {e}"))?;
    }

    tracing::info!(
        url = %clone_url,
        cache = %cache_dir.display(),
        "[memory_sources:github:git] cloning bare repo"
    );

    let output = tokio::time::timeout(
        GIT_CLONE_TIMEOUT,
        tokio::process::Command::new("git")
            .args(["clone", "--bare", "--quiet", clone_url])
            .arg(cache_dir)
            .output(),
    )
    .await
    .map_err(|_| "git clone timed out".to_string())?
    .map_err(|e| format!("git clone failed: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git clone exited {}: {stderr}", output.status));
    }

    Ok(())
}

/// List commits in the bare clone, newest first, up to `max`.
///
/// `branch` restricts the walk to a single ref (default `HEAD` — the bare
/// clone's default branch, matching the REST fallback's default-branch
/// scope), and `paths` narrows it to commits touching any of the given
/// pathspecs.
pub(super) async fn list_commits_git(
    owner: &str,
    repo: &str,
    max: u32,
    cache_dir: &Path,
    branch: Option<&str>,
    paths: &[String],
) -> Result<Vec<SourceItem>, String> {
    ensure_bare_clone(owner, repo, cache_dir).await?;

    let args = log_args(max, branch, paths);

    let output = tokio::time::timeout(
        GIT_LOG_TIMEOUT,
        tokio::process::Command::new("git")
            .args(&args)
            .current_dir(cache_dir)
            .output(),
    )
    .await
    .map_err(|_| "git log timed out".to_string())?
    .map_err(|e| format!("git log failed: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git log exited {}: {stderr}", output.status));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let items: Vec<SourceItem> = stdout
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let parts: Vec<&str> = line.splitn(3, '\t').collect();
            let sha = parts.first().unwrap_or(&"");
            let subject = parts.get(1).unwrap_or(&"");
            let date = parts.get(2).unwrap_or(&"");
            SourceItem {
                id: format!("commit:{sha}"),
                title: subject.to_string(),
                updated_at_ms: parse_iso_ts(date),
            }
        })
        .collect();

    tracing::debug!(
        count = items.len(),
        "[memory_sources:github:git] listed commits via local git"
    );
    Ok(items)
}

/// Build the `git log` argument list for the commit walk.
///
/// `branch` restricts the walk to a single ref (default `HEAD` — the bare
/// clone's default branch, matching the REST fallback's default-branch
/// scope), and `paths` narrows it to commits touching any of the given
/// pathspecs (trailing `-- path1 path2`). Extracted as a pure helper so the
/// filter wiring is unit-testable without a real clone.
pub(super) fn log_args(max: u32, branch: Option<&str>, paths: &[String]) -> Vec<String> {
    let mut args: Vec<String> = vec!["log".to_string()];
    match branch {
        Some(b) if !b.is_empty() => args.push(b.to_string()),
        _ => args.push("HEAD".to_string()),
    }
    args.push(format!("--max-count={max}"));
    args.push("--format=%H\t%s\t%aI".to_string());
    if !paths.is_empty() {
        args.push("--".to_string());
        args.extend(paths.iter().cloned());
    }
    args
}

/// Read one commit's full message and metadata from the bare clone.
pub(super) async fn read_commit_git(
    owner: &str,
    repo: &str,
    sha: &str,
    cache_dir: &Path,
) -> Result<SourceContent, String> {
    if !cache_dir.join("HEAD").exists() {
        return Err("bare clone not present".to_string());
    }

    // git show with a custom format for author, date, and full message.
    let output = tokio::time::timeout(
        GIT_LOG_TIMEOUT,
        tokio::process::Command::new("git")
            .args(["show", "--no-patch", "--format=%H%n%aN%n%aE%n%aI%n%B", sha])
            .current_dir(cache_dir)
            .output(),
    )
    .await
    .map_err(|_| "git show timed out".to_string())?
    .map_err(|e| format!("git show failed: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git show exited {}: {stderr}", output.status));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut lines = stdout.lines();
    let full_sha = lines.next().unwrap_or(sha);
    let author_name = lines.next().unwrap_or("unknown");
    let author_email = lines.next().unwrap_or("");
    let date = lines.next().unwrap_or("unknown");
    let message: String = lines.collect::<Vec<&str>>().join("\n");
    let message = message.trim();

    let title = message.lines().next().unwrap_or("").to_string();
    let author = format!("{author_name} <{author_email}>");

    let body = format!(
        "# Commit: {title}\n\n\
         **SHA:** {full_sha}\n\
         **Author:** {author}\n\
         **Date:** {date}\n\n\
         ## Message\n\n\
         {message}",
    );

    Ok(SourceContent {
        id: format!("commit:{sha}"),
        title,
        body,
        content_type: ContentType::Markdown,
        metadata: serde_json::json!({
            "owner": owner,
            "repo": repo,
            "sha": full_sha,
            "author": author,
        }),
    })
}

#[cfg(test)]
#[path = "git_tests.rs"]
mod tests;
