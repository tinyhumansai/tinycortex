//! Source readers: the [`SourceReader`] trait plus local implementations.
//!
//! A reader knows how to *list* the items available in a source and *read* the
//! content of one item. The trait is intentionally narrow so the host can drive
//! ingestion uniformly across every source kind.
//!
//! ## Ownership boundary
//!
//! Per the engine spec, TinyCortex does **not** own live sync, polling, or
//! OAuth. The network-backed kinds (`composio`, `github_repo`, `rss_feed`,
//! `web_page`, `twitter_query`) keep their type contracts and validation in
//! [`crate::memory::sources::types`] / [`crate::memory::sources::validation`],
//! but their live fetchers are host-owned and are deliberately not implemented
//! here. Only the local kinds — [`folder::FolderReader`] and
//! [`conversation::ConversationReader`] — ship real readers.
//!
//! [`reader_for`] returns `Some` only for the locally-readable kinds; callers
//! handling a `None` should defer to the host's sync runner.

pub mod conversation;
pub mod folder;

use async_trait::async_trait;

use crate::memory::config::MemoryConfig;
use crate::memory::error::MemoryEngineResult;

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

/// Whether a kind has a local reader implemented in TinyCortex.
///
/// Network-backed kinds return `false`: their live fetchers are host-owned.
pub fn is_locally_readable(kind: &SourceKind) -> bool {
    matches!(kind, SourceKind::Folder | SourceKind::Conversation)
}

/// Get the local reader for a source kind, if one exists in TinyCortex.
///
/// Returns `Some` for [`SourceKind::Folder`] and [`SourceKind::Conversation`].
/// For network-backed kinds (`composio`, `github_repo`, `rss_feed`,
/// `web_page`, `twitter_query`) this returns `None` — those are read by the
/// host's sync runner, not TinyCortex.
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
