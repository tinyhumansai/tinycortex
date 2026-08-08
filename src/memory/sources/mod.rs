//! Memory sources — the registry of connectors that feed memory.
//!
//! This domain owns the **"what feeds my memory?"** question: a typed registry
//! of sources (Composio OAuth connections, local folders, GitHub repos, RSS
//! feeds, Twitter queries, web pages, and agent conversations) persisted in
//! `config.toml` under `[[memory_sources]]`.
//!
//! It provides:
//! - [`types`]: the [`MemorySourceEntry`] contract, [`SourceKind`] discriminator,
//!   and reader output types ([`SourceItem`], [`SourceContent`], [`ContentType`]).
//! - [`validation`]: required-field rules per kind and the shared
//!   path-traversal guard for local readers.
//! - [`registry`]: a TOML-backed [`SourceRegistry`] with the
//!   load/modify/validate/save CRUD cycle.
//! - [`readers`]: the [`SourceReader`] trait and the local folder/conversation
//!   reader implementations.
//!
//! ## Ownership boundary
//!
//! TinyCortex owns fetching and parsing — `github_repo`, `rss_feed`, and
//! `web_page` ship readers here behind the `sync` feature — but it does **not**
//! own live sync scheduling, polling cadence, OAuth, or credentials. The host's
//! sync runner consumes this registry to decide what to sync and when, and
//! [`readers::reader_for`] hands out only the two kinds (`folder`,
//! `conversation`) that are safe to read on a timer with no network egress.

pub mod readers;
pub mod registry;
pub mod types;
pub mod validation;

pub use readers::{is_locally_readable, reader_for, SourceReader};
pub use registry::{memory_sync_defaults_for_toolkit, ComposioUpsertTarget, SourceRegistry};
pub use types::{
    ContentType, MemorySourceEntry, MemorySourcePatch, SourceContent, SourceItem, SourceKind,
};
pub use validation::{ensure_within_base, validate_entry};
