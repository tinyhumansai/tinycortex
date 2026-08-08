//! Source readers: the [`SourceReader`] trait plus local implementations.
//!
//! A reader knows how to *list* the items available in a source and *read* the
//! content of one item. The trait is intentionally narrow so the host can drive
//! ingestion uniformly across every source kind.
//!
//! ## Ownership boundary
//!
//! Fetching and parsing a source is engine work, so the `github_repo`,
//! `rss_feed`, and `web_page` readers live here behind the `sync` feature
//! alongside the always-compiled local kinds ([`folder::FolderReader`],
//! [`conversation::ConversationReader`]). What TinyCortex still does **not**
//! own is *when* a network read happens: scheduling, polling cadence, OAuth,
//! credentials, and egress/cost budgeting stay with the host.
//!
//! That is why [`reader_for`] and [`is_locally_readable`] draw their line at
//! **local vs. network**, not at implemented vs. absent. A network reader is
//! constructed explicitly (`github::GithubReader`, `rss::RssReader`,
//! `web_page::WebPageReader`) by a caller that has already decided the fetch is
//! allowed; it is never handed out by the kind-dispatch that
//! [`crate::memory::sync::workspace`] drives on a timer. A `None` from
//! [`reader_for`] therefore still means "route this through the host's sync
//! runner", which is what keeps the host in charge of hitting the network.
//!
//! `composio` and `twitter_query` have no reader here at all — the former is a
//! credentialed OAuth pipeline, the latter is unimplemented.

pub mod conversation;
pub mod folder;
#[cfg(feature = "sync")]
pub mod github;
#[cfg(feature = "sync")]
pub mod rss;
#[cfg(feature = "sync")]
pub mod web_page;

use async_trait::async_trait;

use crate::memory::config::MemoryConfig;
use crate::memory::error::MemoryEngineResult;
#[cfg(feature = "sync")]
use crate::memory::error::MemoryError;

use super::types::{MemorySourceEntry, SourceContent, SourceItem, SourceKind};

/// A reader that can list items and read content from a memory source.
///
/// Implementations are synchronous internally but expose an async surface so a
/// network-backed reader (host-owned) can satisfy the same contract.
#[async_trait]
pub trait SourceReader: Send + Sync {
    /// The [`SourceKind`] this reader serves.
    fn kind(&self) -> SourceKind;

    /// List the items currently available in `source`.
    async fn list_items(
        &self,
        source: &MemorySourceEntry,
        config: &MemoryConfig,
    ) -> MemoryEngineResult<Vec<SourceItem>>;

    /// Read the content of a single item by its reader-scoped `item_id`.
    async fn read_item(
        &self,
        source: &MemorySourceEntry,
        item_id: &str,
        config: &MemoryConfig,
    ) -> MemoryEngineResult<SourceContent>;
}

/// Whether a kind can be read from local state alone, with no network egress.
///
/// Network-backed kinds return `false` even when this build ships their reader
/// (see the module docs): the host decides when a fetch is allowed.
pub fn is_locally_readable(kind: &SourceKind) -> bool {
    matches!(kind, SourceKind::Folder | SourceKind::Conversation)
}

/// Get the reader for a source kind that is safe to drive on a timer.
///
/// Returns `Some` for [`SourceKind::Folder`] and [`SourceKind::Conversation`].
/// Network-backed kinds (`composio`, `github_repo`, `rss_feed`, `web_page`,
/// `twitter_query`) return `None` so the caller defers to the host's sync
/// runner — including the three whose readers this crate now implements, which
/// callers construct by name once the host has authorized the fetch.
pub fn reader_for(kind: &SourceKind) -> Option<Box<dyn SourceReader>> {
    match kind {
        SourceKind::Folder => Some(Box::new(folder::FolderReader)),
        SourceKind::Conversation => Some(Box::new(conversation::ConversationReader)),
        SourceKind::Composio
        | SourceKind::GithubRepo
        | SourceKind::TwitterQuery
        | SourceKind::RssFeed
        | SourceKind::WebPage => None,
    }
}

/// Wrap a reader's plain-string failure as a [`MemoryError`].
///
/// The network readers below carry their diagnostics as `String` internally.
/// [`MemoryError::Other`] is `#[error(transparent)]`, so `to_string()` on the
/// result reproduces the original message byte-for-byte — callers that match on
/// reader error text keep working unchanged.
#[cfg(feature = "sync")]
pub(crate) fn into_engine_error(message: String) -> MemoryError {
    MemoryError::Other(anyhow::anyhow!(message))
}
