//! GitHub repo source reader.
//!
//! Pulls **project activity** (commits, issues, PRs) from a GitHub
//! repository — not source code. Uses the `gh` CLI when available for
//! authenticated, higher-rate-limit access; falls back to the public
//! GitHub REST API for unauthenticated reads.
//!
//! ## Module layout
//!
//! - [`self`] — [`GithubReader`] orchestration: item listing/reading, URL
//!   parsing, raw-archive coordinates, shared utilities, and the cached
//!   `gh`-availability probe.
//! - `types` — API response models and the `gh`-fallback list cache.
//! - `git` — local bare-clone + `git log` / `git show` helpers.
//! - `api` — `gh api` / REST transport plus commit list/read helpers.
//! - `issues` — issue and pull-request list/read helpers.

mod api;
mod git;
mod issues;
mod types;

#[cfg(test)]
#[path = "github_tests.rs"]
mod tests;

use std::time::Duration;

use async_trait::async_trait;

use crate::memory::config::MemoryConfig;
use crate::memory::error::MemoryEngineResult;
use crate::memory::sources::types::{MemorySourceEntry, SourceContent, SourceItem, SourceKind};
use crate::memory::store::content::raw::RawKind;

use super::{into_engine_error, SourceReader};

// Re-export for the sibling submodules and the test module.
pub(crate) use types::{ItemKind, LIST_CACHE};

/// Default number of items of **each** type (commits, issues, PRs) to pull
/// when the source entry doesn't override it. Tunable per-source via
/// `max_commits` / `max_issues` / `max_prs` on [`MemorySourceEntry`].
pub(crate) const DEFAULT_GITHUB_ITEM_LIMIT: u32 = 1000;

/// Timeout for a single `gh` CLI invocation (including the availability
/// probe).
const GH_CLI_TIMEOUT: Duration = Duration::from_secs(30);

/// Whether the `gh` CLI is on PATH and runs. Probed once per process and
/// cached: `gh api` is the preferred transport for authenticated,
/// higher-rate-limit access, and re-probing on every item read is wasteful.
static GH_AVAILABLE: tokio::sync::OnceCell<bool> = tokio::sync::OnceCell::const_new();

/// Probe `gh --version` (async, so a stuck `gh` cannot block a worker
/// thread) and cache the result for the process lifetime.
async fn gh_available() -> bool {
    *GH_AVAILABLE
        .get_or_init(|| async {
            let status = tokio::time::timeout(
                GH_CLI_TIMEOUT,
                tokio::process::Command::new("gh")
                    .arg("--version")
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status(),
            )
            .await;
            status
                .map(|s| s.map(|st| st.success()).unwrap_or(false))
                .unwrap_or(false)
        })
        .await
}

pub struct GithubReader;

/// Parse `owner` and `repo` from a GitHub URL.
///
/// Accepts only the canonical `https://github.com/<owner>/<repo>[.git][/]`
/// shape — extra segments like `/tree/main` or `/blob/...` are rejected
/// so callers can't accidentally derive the wrong owner/repo from a
/// deep link.
pub(crate) fn parse_github_url(url: &str) -> Result<(String, String), String> {
    let trimmed = url.trim();
    let rest = trimmed
        .strip_prefix("https://github.com/")
        .or_else(|| trimmed.strip_prefix("http://github.com/"))
        .or_else(|| trimmed.strip_prefix("git@github.com:"))
        .ok_or_else(|| format!("not a GitHub URL: {url}"))?;
    let cleaned = rest.trim_end_matches('/').trim_end_matches(".git");
    let parts: Vec<&str> = cleaned.split('/').collect();
    if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
        return Err(format!(
            "expected https://github.com/<owner>/<repo>, got: {url}"
        ));
    }
    Ok((parts[0].to_string(), parts[1].to_string()))
}

// ── Raw-archive coordinates ─────────────────────────────────────────

/// Slugifiable raw-archive source id for a repo URL.
///
/// Returns `github.com/<owner>/<repo>`, which slugifies (via
/// `slugify_source_id`) to `github-com-<owner>-<repo>` so a source's
/// commits/issues/PRs land under
/// `raw/github-com-<owner>-<repo>/{commits,issues,prs}/`.
pub fn repo_archive_source_id(url: &str) -> Option<String> {
    let (owner, repo) = parse_github_url(url).ok()?;
    Some(format!("github.com/{owner}/{repo}"))
}

/// Chunk-store source id for a single repo item (dedup key).
///
/// `github:<owner>/<repo>:<item_id>` keeps per-item uniqueness for the
/// `mem_tree_ingested_sources` dedup table while the separate
/// [`repo_chunk_scope`] drives a shared directory.
pub fn chunk_source_id(url: &str, item_id: &str) -> Option<String> {
    let (owner, repo) = parse_github_url(url).ok()?;
    Some(format!("github:{owner}/{repo}:{item_id}"))
}

/// Repo-scoped chunk path scope so all items from one repo share a
/// single directory in the content store (e.g. `document/github-org-repo/`).
pub fn repo_chunk_scope(url: &str) -> Option<String> {
    let (owner, repo) = parse_github_url(url).ok()?;
    Some(format!("github:{owner}/{repo}"))
}

/// Map a [`SourceItem`] id (`commit:<sha>`, `issue:<n>`, `pr:<n>`) to its
/// raw-archive [`RawKind`] and the clean uid used as the filename suffix.
pub fn raw_archive_coords(item_id: &str) -> Option<(RawKind, String)> {
    let (kind, rest) = ItemKind::from_id(item_id)?;
    let raw_kind = match kind {
        ItemKind::Commit => RawKind::Commit,
        ItemKind::Issue => RawKind::Issue,
        ItemKind::PullRequest => RawKind::PullRequest,
    };
    Some((raw_kind, rest.to_string()))
}

// ── Reader implementation ───────────────────────────────────────────

#[async_trait]
impl SourceReader for GithubReader {
    fn kind(&self) -> SourceKind {
        SourceKind::GithubRepo
    }

    async fn list_items(
        &self,
        source: &MemorySourceEntry,
        config: &MemoryConfig,
    ) -> MemoryEngineResult<Vec<SourceItem>> {
        self.list_items_inner(source, config)
            .await
            .map_err(into_engine_error)
    }

    async fn read_item(
        &self,
        source: &MemorySourceEntry,
        item_id: &str,
        config: &MemoryConfig,
    ) -> MemoryEngineResult<SourceContent> {
        self.read_item_inner(source, item_id, config)
            .await
            .map_err(into_engine_error)
    }
}

impl GithubReader {
    async fn list_items_inner(
        &self,
        source: &MemorySourceEntry,
        config: &MemoryConfig,
    ) -> Result<Vec<SourceItem>, String> {
        let url = source
            .url
            .as_deref()
            .ok_or("github source requires a url")?;
        let (owner, repo) = parse_github_url(url)?;
        let use_gh = gh_available().await;

        let max_commits = source.max_commits.unwrap_or(DEFAULT_GITHUB_ITEM_LIMIT);
        let max_issues = source.max_issues.unwrap_or(DEFAULT_GITHUB_ITEM_LIMIT);
        let max_prs = source.max_prs.unwrap_or(DEFAULT_GITHUB_ITEM_LIMIT);
        // A configured branch narrows commits to that ref; configured paths
        // narrow them to the touched files. Both fall through to the API
        // fallback so the two transports agree on scope.
        let branch = source.branch.as_deref();
        let paths = source.paths.as_slice();

        let cache_dir = git::git_cache_dir(&config.workspace, &owner, &repo);

        tracing::debug!(
            owner = %owner,
            repo = %repo,
            use_gh = use_gh,
            branch = %branch.unwrap_or("(all)"),
            max_commits,
            max_issues,
            max_prs,
            cache = %cache_dir.display(),
            "[memory_sources:github] listing items"
        );

        // Clear the list cache so stale data from a prior sync doesn't
        // leak into this run.
        if let Ok(mut cache) = LIST_CACHE.lock() {
            cache.clear();
        }

        let mut items = Vec::new();
        let mut errors = Vec::new();

        // Commits via local git (clone/fetch bare repo, then git log)
        match git::list_commits_git(&owner, &repo, max_commits, &cache_dir, branch, paths).await {
            Ok(commits) => items.extend(commits),
            Err(e) => {
                tracing::warn!(error = %e, "[memory_sources:github] git commit list failed, falling back to API");
                match api::list_commits_api(&owner, &repo, max_commits, use_gh, branch, paths).await
                {
                    Ok(commits) => items.extend(commits),
                    Err(e2) => {
                        tracing::warn!(error = %e2, "[memory_sources:github] API commit list also failed");
                        errors.push(e2);
                    }
                }
            }
        }

        // Issues and PRs via gh CLI / API (no local equivalent)
        match issues::list_issues(&owner, &repo, max_issues, use_gh).await {
            Ok(issues) => items.extend(issues),
            Err(e) => {
                tracing::warn!(error = %e, "[memory_sources:github] failed to list issues");
                errors.push(e);
            }
        }

        match issues::list_prs(&owner, &repo, max_prs, use_gh).await {
            Ok(prs) => items.extend(prs),
            Err(e) => {
                tracing::warn!(error = %e, "[memory_sources:github] failed to list PRs");
                errors.push(e);
            }
        }

        if items.is_empty() && !errors.is_empty() {
            return Err(format!(
                "all GitHub API calls failed: {}",
                errors.join("; ")
            ));
        }

        tracing::debug!(count = items.len(), "[memory_sources:github] found items");
        Ok(items)
    }

    async fn read_item_inner(
        &self,
        source: &MemorySourceEntry,
        item_id: &str,
        config: &MemoryConfig,
    ) -> Result<SourceContent, String> {
        let url = source
            .url
            .as_deref()
            .ok_or("github source requires a url")?;
        let (owner, repo) = parse_github_url(url)?;
        let use_gh = gh_available().await;

        let (kind, ref_id) =
            ItemKind::from_id(item_id).ok_or_else(|| format!("invalid item id: {item_id}"))?;

        tracing::debug!(
            item_id = %item_id,
            kind = ?kind,
            "[memory_sources:github] reading item"
        );

        match kind {
            ItemKind::Commit => {
                let cache_dir = git::git_cache_dir(&config.workspace, &owner, &repo);
                match git::read_commit_git(&owner, &repo, ref_id, &cache_dir).await {
                    Ok(content) => Ok(content),
                    Err(e) => {
                        tracing::debug!(
                            sha = %ref_id,
                            error = %e,
                            "[memory_sources:github] git read_commit failed, falling back to API"
                        );
                        api::read_commit_api(&owner, &repo, ref_id, use_gh).await
                    }
                }
            }
            ItemKind::Issue => {
                let num: u64 = ref_id
                    .parse()
                    .map_err(|_| format!("invalid issue number: {ref_id}"))?;
                issues::read_issue(&owner, &repo, num, use_gh).await
            }
            ItemKind::PullRequest => {
                let num: u64 = ref_id
                    .parse()
                    .map_err(|_| format!("invalid PR number: {ref_id}"))?;
                issues::read_pr(&owner, &repo, num, use_gh).await
            }
        }
    }
}

// ── Utilities ───────────────────────────────────────────────────────

fn parse_iso_ts(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp_millis())
}

/// Render GitHub logins as a deduped, order-preserving, space-separated
/// list of `@handle`s. Empty / `unknown` logins are skipped; an empty
/// result renders as `none`. Used so unique committers/commenters surface
/// as `handle:` entities in the memory tree.
fn unique_handles<'a>(logins: impl Iterator<Item = &'a str>) -> String {
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<String> = Vec::new();
    for login in logins {
        let l = login.trim();
        if l.is_empty() || l == "unknown" {
            continue;
        }
        if seen.insert(l.to_string()) {
            out.push(format!("@{l}"));
        }
    }
    if out.is_empty() {
        "none".to_string()
    } else {
        out.join(" ")
    }
}
