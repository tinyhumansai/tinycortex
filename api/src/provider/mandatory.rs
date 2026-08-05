//! The three mandatory capability families: [`MemoryCore`], [`MemoryRecall`],
//! and [`MemoryPortability`].
//!
//! These are supertraits of [`crate::provider::MemoryProvider`], which is what
//! makes "mandatory" a *compile-time* fact rather than a runtime check: a type
//! that does not implement all three cannot be a provider at all, so there is
//! no way to bind a driver that is missing them.
//!
//! The other ten families are reached through `Option`-returning accessors on
//! the provider, so their absence is representable and their presence is not
//! assumed. See [`crate::provider::MemoryProvider`] for that half.
//!
//! ## Why every method returns [`MemoryError`] and not `anyhow::Error`
//!
//! The transport adapter must be able to turn a `501` from an out-of-process
//! driver into [`MemoryError::Unsupported`], and the kernel must be able to
//! tell "this driver cannot do that" apart from "this driver failed". An
//! `anyhow::Error` erases exactly that distinction. The engine's own
//! [`crate::traits::Memory`] trait keeps `anyhow::Result` — it is an internal
//! storage abstraction with existing implementors, not the driver contract.

use async_trait::async_trait;

use crate::error::MemoryError;
use crate::provider::types::{ExportPage, ExportRecord, ImportOutcome, SourceScope};
use crate::recall::OwnedRecallOpts;
use crate::types::{MemoryCategory, MemoryEntry, MemoryTaint, NamespaceSummary};

/// Store, read, and delete individual memory entries. **Mandatory.**
///
/// This is the smallest surface that still makes something a memory backend:
/// without it there is nothing to recall from and nothing to export.
#[async_trait]
pub trait MemoryCore: Send + Sync {
    /// Upsert an entry, keyed by `(namespace, key)`.
    ///
    /// ## Taint is an argument, never a decision
    ///
    /// Unlike the engine's [`crate::traits::Memory`], which has a `store` and a
    /// separate `store_with_taint` whose default implementation silently drops
    /// the taint, the contract has **one** store and it always takes a
    /// [`MemoryTaint`]. Provenance is stamped by the host policy guard before
    /// the call; a driver that could default it would be able to launder
    /// externally-sourced content into internal-trust content, which is the
    /// single failure mode the guard exists to prevent.
    ///
    /// # Errors
    ///
    /// [`MemoryError::Invalid`] for caller input the driver rejects,
    /// [`MemoryError::Io`] or [`MemoryError::Other`] for backend failures.
    async fn store(
        &self,
        namespace: &str,
        key: &str,
        content: &str,
        category: MemoryCategory,
        session_id: Option<&str>,
        taint: MemoryTaint,
    ) -> Result<(), MemoryError>;

    /// Fetch the entry for an exact `(namespace, key)`.
    ///
    /// # Errors
    ///
    /// A missing entry is `Ok(None)`, never an error; `Err` is reserved for
    /// backend failures.
    async fn get(&self, namespace: &str, key: &str) -> Result<Option<MemoryEntry>, MemoryError>;

    /// Delete the entry for `(namespace, key)`, reporting whether it existed.
    ///
    /// Idempotent: forgetting an absent key is `Ok(false)`, so callers may call
    /// it unconditionally.
    ///
    /// # Errors
    ///
    /// Backend failures only.
    async fn forget(&self, namespace: &str, key: &str) -> Result<bool, MemoryError>;

    /// List entries, narrowing by namespace, category, and session.
    ///
    /// Each `Some` filter narrows the result; all `None` lists everything the
    /// driver holds. An empty result is `Ok(vec![])`.
    ///
    /// # Errors
    ///
    /// Backend failures only.
    async fn list(
        &self,
        namespace: Option<&str>,
        category: Option<&MemoryCategory>,
        session_id: Option<&str>,
    ) -> Result<Vec<MemoryEntry>, MemoryError>;

    /// Enumerate namespaces with their aggregate counts, for discovery.
    ///
    /// # Errors
    ///
    /// Backend failures only.
    async fn namespaces(&self) -> Result<Vec<NamespaceSummary>, MemoryError>;
}

/// Ranked retrieval. **Mandatory.**
#[async_trait]
pub trait MemoryRecall: Send + Sync {
    /// Return up to `limit` entries relevant to `query`, most relevant first.
    ///
    /// `opts` is the **owned** [`OwnedRecallOpts`], never the borrowed
    /// `RecallOpts<'a>`: a lifetime parameter cannot travel through an
    /// object-safe `#[async_trait]` method, and the borrowed form derives no
    /// serde impls so it could never be a request body. An embedded driver
    /// converts to the borrowed form at its own boundary, which is zero-copy.
    ///
    /// `scope` is the per-turn source allowlist and is a **query predicate the
    /// driver must apply internally** — see [`SourceScope`] for why applying it
    /// after the fact is wrong. `None` means unrestricted.
    ///
    /// An empty or non-matching `query` yields `Ok(vec![])`, not an error.
    ///
    /// # Errors
    ///
    /// [`MemoryError::Invalid`] for a malformed filter, otherwise backend
    /// failures.
    async fn recall(
        &self,
        query: &str,
        limit: usize,
        opts: &OwnedRecallOpts,
        scope: Option<&SourceScope>,
    ) -> Result<Vec<MemoryEntry>, MemoryError>;
}

/// Export and import the whole store. **Mandatory.**
///
/// Mandatory because binding a memory backend without it is a one-way door: a
/// user who cannot export cannot leave. It is the capability that makes every
/// other binding reversible, which is also why the `mirror` migration driver is
/// expressible at all.
#[async_trait]
pub trait MemoryPortability: Send + Sync {
    /// Read one page of the export, continuing from `cursor`.
    ///
    /// Pass `None` to start. The export is complete when the returned
    /// [`ExportPage::next_cursor`] is `None` — an empty `records` vector is
    /// **not** a terminator, because a driver may legitimately return an empty
    /// page while skipping a range.
    ///
    /// `limit` is a request, not a guarantee; a driver may return fewer.
    ///
    /// ## Why pages and not a stream
    ///
    /// A `Stream` return type would either make the trait non-object-safe or
    /// drag an async runtime into a crate that deliberately has none. Paging
    /// keeps both properties and still bounds memory, with the caller choosing
    /// the bound.
    ///
    /// # Errors
    ///
    /// [`MemoryError::Invalid`] for a cursor this driver did not issue,
    /// otherwise backend failures.
    async fn export_page(
        &self,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<ExportPage, MemoryError>;

    /// Write a batch of previously-exported records.
    ///
    /// Records carry their own [`crate::types::MemoryTaint`]; an importing
    /// driver must persist what it is given and must not re-stamp provenance.
    ///
    /// Partial success is normal and is reported in [`ImportOutcome`] rather
    /// than as an error: a migration should not abort a million-record restore
    /// because one record was malformed.
    ///
    /// # Errors
    ///
    /// Reserved for failures that make the whole batch meaningless (backend
    /// unavailable, transaction aborted). Per-record rejection belongs in
    /// [`ImportOutcome::failed`].
    async fn import_records(
        &self,
        records: Vec<ExportRecord>,
    ) -> Result<ImportOutcome, MemoryError>;
}
