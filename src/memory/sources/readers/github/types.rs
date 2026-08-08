//! API response models and the `gh`-fallback list cache for the GitHub
//! reader. Pure data — no I/O lives here. Models are deliberately kept loose
//! (only the fields the reader consumes are declared) so new GitHub response
//! fields don't force a struct change.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use serde::Deserialize;

/// A commit object from the REST commits endpoint.
#[derive(Debug, Deserialize)]
pub(crate) struct GhCommit {
    pub(crate) sha: String,
    pub(crate) commit: GhCommitInner,
    /// Top-level GitHub user that authored the commit (distinct from the
    /// embedded git author identity). Present when the commit author maps
    /// to a GitHub account; absent for unlinked email-only authors.
    #[serde(default)]
    pub(crate) author: Option<GhUser>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct GhCommitInner {
    pub(crate) message: String,
    pub(crate) author: Option<GhAuthor>,
    pub(crate) committer: Option<GhAuthor>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct GhAuthor {
    pub(crate) name: Option<String>,
    pub(crate) email: Option<String>,
    pub(crate) date: Option<String>,
}

/// An issue list entry (`GET /repos/{owner}/{repo}/issues`).
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct GhIssue {
    pub(crate) number: u64,
    pub(crate) title: String,
    pub(crate) body: Option<String>,
    pub(crate) state: String,
    pub(crate) user: Option<GhUser>,
    pub(crate) labels: Vec<GhLabel>,
    pub(crate) created_at: Option<String>,
    pub(crate) updated_at: Option<String>,
    /// Present when the row is actually a pull request (the issues endpoint
    /// returns PRs with a `pull_request` envelope).
    pub(crate) pull_request: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct GhUser {
    pub(crate) login: String,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct GhLabel {
    pub(crate) name: String,
}

/// A pull request list entry.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct GhPr {
    pub(crate) number: u64,
    pub(crate) title: String,
    pub(crate) body: Option<String>,
    pub(crate) state: String,
    pub(crate) user: Option<GhUser>,
    pub(crate) labels: Vec<GhLabel>,
    pub(crate) created_at: Option<String>,
    pub(crate) updated_at: Option<String>,
    pub(crate) merged_at: Option<String>,
}

/// What kind of GitHub item a list row refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ItemKind {
    Commit,
    Issue,
    PullRequest,
}

impl ItemKind {
    /// Parse a `SourceItem` id (`commit:<sha>`, `issue:<n>`, `pr:<n>`) into
    /// its kind and the ref (sha / number) that follows the prefix.
    pub(crate) fn from_id(id: &str) -> Option<(Self, &str)> {
        if let Some(rest) = id.strip_prefix("commit:") {
            Some((ItemKind::Commit, rest))
        } else if let Some(rest) = id.strip_prefix("issue:") {
            Some((ItemKind::Issue, rest))
        } else if let Some(rest) = id.strip_prefix("pr:") {
            Some((ItemKind::PullRequest, rest))
        } else {
            None
        }
    }
}

/// A cached issue/PR row, keyed by its list id (`"<owner>/<repo>:<item_id>"`).
/// The issues endpoint returns pull requests mixed in with issues, and the PR
/// endpoint is the only one that returns merge state, so the list pass stashes
/// the full row here and the read pass reuses it instead of re-fetching.
#[derive(Debug, Clone)]
pub(crate) enum CachedItem {
    Issue(GhIssue),
    Pr(GhPr),
}

/// Process-wide cache of issue/PR list rows, cleared at the start of each
/// `list_items` run so stale data from a prior sync can't leak in.
pub(crate) static LIST_CACHE: LazyLock<Mutex<HashMap<String, CachedItem>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
