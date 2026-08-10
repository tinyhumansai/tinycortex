//! The remaining optional families: [`MemoryGoals`], [`MemoryToolMemory`],
//! [`MemorySourceSink`], and [`MemoryMaintenance`].
//!
//! Goals and tool memory are small curated record sets the agent reads on
//! nearly every turn. The source sink is the seam the host's sync machinery
//! writes through. Maintenance is the seam the host's scheduler drives.
//!
//! ## The host keeps the loop; the driver runs one step
//!
//! [`MemorySourceSink`] receives already-fetched items — the host owns
//! credentials, OAuth, rate limits, and the schedule. [`MemoryMaintenance`]
//! exposes four operations the host's existing scheduler calls; no driver
//! installs a background task of its own. Both follow the same rule as the
//! engine's `queue::run_once`, and both are why a driver never needs to see
//! configuration or a keychain.

use async_trait::async_trait;

use crate::error::MemoryError;
use crate::goals::GoalsDoc;
use crate::provider::types::{IngestOutcome, MaintenanceReport, SourceItem};
use crate::tool_memory::ToolMemoryRule;
use crate::types::MemoryTaint;

/// The agent's long-term goals document.
#[async_trait]
pub trait MemoryGoals: Send + Sync {
    /// Read the current goals document.
    ///
    /// A driver with no goals yet returns an empty [`GoalsDoc`], not
    /// [`MemoryError::NotFound`] — "no goals" is a valid state, not a missing
    /// record.
    ///
    /// # Errors
    ///
    /// Backend failures only.
    async fn goals(&self) -> Result<GoalsDoc, MemoryError>;

    /// Replace the goals document wholesale.
    ///
    /// Whole-document replacement rather than per-item add/edit/delete because
    /// the validating mutation surface (PII and secret predicates) is **host**
    /// policy: the host parses, validates, mutates, and hands back the result.
    /// Exposing per-item mutation here would put that policy behind a trait a
    /// third-party driver implements, where it could be skipped.
    ///
    /// # Errors
    ///
    /// [`MemoryError::Invalid`] for a document the driver refuses (e.g. over
    /// its own item cap), otherwise backend failures.
    async fn set_goals(&self, goals: GoalsDoc) -> Result<(), MemoryError>;
}

/// Per-tool learned rules — durable guidance attached to a specific tool.
#[async_trait]
pub trait MemoryToolMemory: Send + Sync {
    /// Rules for one tool, highest priority first.
    ///
    /// # Errors
    ///
    /// Backend failures only; a tool with no rules yields an empty vector.
    async fn tool_rules(&self, tool_name: &str) -> Result<Vec<ToolMemoryRule>, MemoryError>;

    /// Upsert one rule, keyed by [`ToolMemoryRule::id`].
    ///
    /// # Errors
    ///
    /// [`MemoryError::Invalid`] for a malformed rule, otherwise backend
    /// failures.
    async fn put_tool_rule(&self, rule: ToolMemoryRule) -> Result<(), MemoryError>;

    /// Delete one rule, reporting whether it existed.
    ///
    /// Idempotent, like [`crate::provider::MemoryCore::forget`].
    ///
    /// # Errors
    ///
    /// Backend failures only.
    async fn delete_tool_rule(&self, tool_name: &str, rule_id: &str) -> Result<bool, MemoryError>;
}

/// The write seam for host-driven source sync.
#[async_trait]
pub trait MemorySourceSink: Send + Sync {
    /// Accept a batch of items the host fetched from one logical source.
    ///
    /// `taint` applies to the whole batch and is stamped by the host. Sync
    /// paths ingesting third-party content pass
    /// [`MemoryTaint::ExternalSync`]; the driver persists what it is given and
    /// never assigns provenance itself.
    ///
    /// `source_kind` is a wire string (`folder`, `composio`, …) rather than an
    /// enum because the set of source kinds is owned by the host's sync
    /// machinery and grows without a contract change.
    ///
    /// # Errors
    ///
    /// [`MemoryError::Invalid`] for a rejected batch, otherwise backend
    /// failures. Per-item outcomes are counted in [`IngestOutcome`].
    async fn accept_source_items(
        &self,
        source_id: &str,
        source_kind: &str,
        items: Vec<SourceItem>,
        taint: MemoryTaint,
    ) -> Result<IngestOutcome, MemoryError>;

    /// Drop everything the driver holds for one logical source, returning how
    /// many units were removed.
    ///
    /// This is the disconnect path: when a user removes a source, its content
    /// must leave memory. Idempotent — an unknown `source_id` returns `Ok(0)`.
    ///
    /// # Errors
    ///
    /// Backend failures only.
    async fn forget_source(&self, source_id: &str) -> Result<u64, MemoryError>;
}

/// Periodic upkeep the host's scheduler drives.
///
/// All four operations must be safe to call repeatedly and safe to interrupt:
/// the scheduler may invoke them on a timer, and a desktop process can exit at
/// any point. A driver that cannot bound the work should do a slice per call
/// and report progress in [`MaintenanceReport`].
#[async_trait]
pub trait MemoryMaintenance: Send + Sync {
    /// Recompute embeddings for content whose embedding is missing or stale.
    ///
    /// # Errors
    ///
    /// Backend failures, or [`MemoryError::BudgetExceeded`] when an embedding
    /// budget is exhausted mid-run.
    async fn reembed(&self) -> Result<MaintenanceReport, MemoryError>;

    /// Reclaim space: vacuum indexes, drop tombstones, prune dead references.
    ///
    /// # Errors
    ///
    /// Backend failures only.
    async fn compact(&self) -> Result<MaintenanceReport, MemoryError>;

    /// Merge and summarise accumulated memory — the "dream" pass.
    ///
    /// The embedded driver maps this onto its seal/cascade/reembed cycle; an
    /// external driver maps it onto whatever it calls the same idea. The
    /// contract deliberately does not specify the mechanism, only that it is
    /// the operation a scheduler runs when the system is idle.
    ///
    /// # Errors
    ///
    /// Backend failures only.
    async fn consolidate(&self) -> Result<MaintenanceReport, MemoryError>;

    /// Read-only integrity check.
    ///
    /// Reports findings in [`MaintenanceReport::findings`] and must change
    /// nothing — [`MaintenanceReport::changed`] is always `0`. A driver that
    /// repairs as it inspects should expose that as [`Self::compact`] instead,
    /// so an operator can diagnose without mutating.
    ///
    /// # Errors
    ///
    /// Backend failures only. A *finding* is not an error: a store with
    /// problems still returns `Ok` with the problems listed.
    async fn doctor(&self) -> Result<MaintenanceReport, MemoryError>;
}
