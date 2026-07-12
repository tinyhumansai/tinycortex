//! Core types for memory sources.
//!
//! A *memory source* answers the question "what feeds my memory?". Each
//! configured source is a [`MemorySourceEntry`] persisted in `config.toml`
//! under `[[memory_sources]]`. The [`SourceKind`] discriminator selects which
//! kind-specific fields are required; required-field checks live in
//! [`crate::memory::sources::validation`] and are surfaced via
//! [`MemorySourceEntry::validate`].
//!
//! Reader output contracts ([`SourceItem`], [`SourceContent`], [`ContentType`])
//! are shared across every reader implementation so the host can ingest source
//! payloads uniformly regardless of where they came from.
//!
//! Wire strings are snake_case and are part of the persisted contract — do not
//! rename them when porting from OpenHuman.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub(crate) fn default_true() -> bool {
    true
}

/// The kind of a configured memory source.
///
/// The wire representation is snake_case (`github_repo`, `rss_feed`, …) and is
/// persisted in `config.toml`; it must stay stable across versions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// A Composio OAuth connector (Gmail, Slack, Notion, …). Network-backed;
    /// the live fetch is owned by the host, not TinyCortex.
    Composio,
    /// Local agent conversation transcripts stored in the workspace.
    Conversation,
    /// A local folder of files matched by an optional glob.
    Folder,
    /// A GitHub repository's project activity (commits, issues, PRs).
    GithubRepo,
    /// A Twitter/X search query.
    TwitterQuery,
    /// An RSS/Atom feed.
    RssFeed,
    /// A single web page, optionally narrowed by a CSS selector.
    WebPage,
}

impl SourceKind {
    /// The stable snake_case wire string for this kind.
    pub fn as_str(&self) -> &'static str {
        match self {
            SourceKind::Composio => "composio",
            SourceKind::Conversation => "conversation",
            SourceKind::Folder => "folder",
            SourceKind::GithubRepo => "github_repo",
            SourceKind::TwitterQuery => "twitter_query",
            SourceKind::RssFeed => "rss_feed",
            SourceKind::WebPage => "web_page",
        }
    }
}

/// A configured memory source entry persisted in `config.toml`.
///
/// All kind-specific fields are flattened onto the struct as `Option`s. The
/// [`kind`](MemorySourceEntry::kind) discriminator determines which fields are
/// required; validation is enforced at add/update time via
/// [`MemorySourceEntry::validate`].
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemorySourceEntry {
    /// Stable unique id (e.g. `src_<uuid>`).
    pub id: String,
    /// Discriminator selecting the kind-specific fields below.
    pub kind: SourceKind,
    /// Human-readable label shown in UIs.
    pub label: String,
    /// Whether this source participates in sync. Defaults to `true`.
    #[serde(default = "default_true")]
    pub enabled: bool,

    // ── Composio ──
    /// Composio toolkit slug (e.g. `gmail`). Required for `composio`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toolkit: Option<String>,
    /// Composio connection id. Required for `composio`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_id: Option<String>,

    // ── Folder ──
    /// Filesystem path of the folder to read. Required for `folder`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Optional glob applied under `path` (defaults to `**/*.md`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glob: Option<String>,

    // ── GithubRepo / RssFeed / WebPage (shared) ──
    /// Source URL. Required for `github_repo`, `rss_feed`, and `web_page`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,

    // ── GithubRepo ──
    /// Branch to read (defaults to the repo default when absent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Optional path filters within the repo.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    /// Max commits to pull per sync (default 1000 when absent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_commits: Option<u32>,
    /// Max issues to pull per sync (default 1000 when absent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_issues: Option<u32>,
    /// Max pull requests to pull per sync (default 1000 when absent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_prs: Option<u32>,

    // ── TwitterQuery ──
    /// Search query. Required for `twitter_query`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// Optional look-back window in days for the query.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since_days: Option<u32>,

    // ── RssFeed ──
    /// Max feed items to pull per sync.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_items: Option<u32>,

    // ── WebPage ──
    /// Optional CSS selector to narrow extracted content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,

    // ── Sync Budget (all source kinds) ──
    /// Maximum tokens to consume per sync run. Sync stops once this budget is hit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens_per_sync: Option<u64>,
    /// Maximum cost in USD per sync run. Refuses LLM calls once reached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cost_per_sync_usd: Option<f64>,
    /// Sync depth in days — only fetch items from the last N days.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync_depth_days: Option<u32>,
}

impl MemorySourceEntry {
    /// Validate required fields for this entry's [`SourceKind`].
    ///
    /// Delegates to [`crate::memory::sources::validation::validate_entry`].
    /// Returns a human-readable error message on the first failing rule.
    pub fn validate(&self) -> Result<(), String> {
        crate::memory::sources::validation::validate_entry(self)
    }
}

/// One item listed from a source reader.
///
/// `id` is reader-scoped (e.g. a folder-relative path or a thread id) and is
/// stable enough to pass back into [`crate::memory::sources::readers::SourceReader::read_item`].
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SourceItem {
    /// Reader-scoped item id.
    pub id: String,
    /// Human-readable title.
    pub title: String,
    /// Last-modified time in epoch milliseconds, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at_ms: Option<i64>,
}

/// The rendered content type of a [`SourceContent`] body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContentType {
    /// Markdown body.
    Markdown,
    /// Raw HTML body.
    Html,
    /// Plain text body.
    Plaintext,
}

/// Content read from a single source item.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SourceContent {
    /// Reader-scoped item id (matches the [`SourceItem::id`] it was read from).
    pub id: String,
    /// Human-readable title.
    pub title: String,
    /// The item body, rendered as [`content_type`](SourceContent::content_type).
    pub body: String,
    /// How [`body`](SourceContent::body) should be interpreted.
    pub content_type: ContentType,
    /// Reader-specific metadata (JSON object).
    #[serde(default)]
    pub metadata: serde_json::Value,
}

#[cfg(test)]
#[path = "types_tests.rs"]
mod tests;
