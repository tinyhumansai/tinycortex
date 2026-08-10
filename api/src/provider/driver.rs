//! [`MemoryProvider`] — the single trait a memory driver implements, and the
//! object the kernel binds.
//!
//! ## Self-contained on purpose
//!
//! `MemoryProvider` does **not** extend a host `Driver` trait and names no host
//! type. `tinycortex-api` is what a third-party driver compiles against, so it
//! must not drag in the OpenHuman host; and the generic subsystem vocabulary
//! (`Driver`, `DriverClass`, `SubsystemRegistry`, the policy `Guard`) belongs
//! kernel-side, where inference and channels can share it without importing a
//! *memory* crate.
//!
//! The bridge is the host's memory adapter, which implements the host `Driver`
//! for an `Arc<dyn MemoryProvider>` and converts [`MemoryHealth`] into the
//! kernel's `DriverHealth`. That conversion is trivial by construction — see
//! [`crate::health`].
//!
//! Driver **class** (embedded / external / null) is deliberately absent from
//! this trait. Class is a fact about how the host bound a driver, recorded in
//! host configuration; a driver self-reporting it would let a misconfigured
//! external backend claim to be embedded and skip the egress and trust checks
//! that class gates.
//!
//! ## The accessor form, and why not `Any`
//!
//! The kernel binds `Arc<dyn MemoryProvider>` and needs per-family access. Two
//! designs were available: downcast through [`std::any::Any`], or one
//! `Option`-returning accessor per optional family. The accessors win:
//!
//! - **No unchecked downcast.** `Any` would require the caller to name a
//!   concrete driver type, which defeats the point of binding behind a trait
//!   object, or to register type ids, which is the same table with worse
//!   ergonomics.
//! - **The capability set and the reachable surface stay provably in sync.**
//!   [`crate::provider::audit_provider`] compares [`MemoryProvider::capabilities`]
//!   against what the accessors actually return, so "advertised but not
//!   implemented" is a detectable, testable mistake instead of a runtime
//!   surprise on the first call.
//! - **[`MemoryProvider::provides`] is an exhaustive `match`** over
//!   [`Capability`], so adding a family without wiring an accessor fails to
//!   compile.
//!
//! The three mandatory families are supertraits rather than accessors, so they
//! are callable directly on the trait object and cannot be absent.
//!
//! ## Object safety
//!
//! Every method here and in every family trait is object-safe: no generic
//! parameters, no `Self` in return position, no associated constants. The
//! `#[async_trait]` attribute rewrites the `async fn`s into boxed futures,
//! which is what makes them dyn-compatible at all.

use async_trait::async_trait;

use crate::capabilities::{Capabilities, Capability};
use crate::error::MemoryError;
use crate::health::MemoryHealth;
use crate::provider::content::{MemoryDocuments, MemoryIngest, MemoryTree};
use crate::provider::knowledge::{MemoryDiff, MemoryEntities, MemoryGraph};
use crate::provider::mandatory::{MemoryCore, MemoryPortability, MemoryRecall};
use crate::provider::records::{
    MemoryGoals, MemoryMaintenance, MemorySourceSink, MemoryToolMemory,
};

/// A bound memory driver.
///
/// Implementors must also implement the three mandatory families
/// ([`MemoryCore`], [`MemoryRecall`], [`MemoryPortability`]) — they are
/// supertraits, so a driver missing any of them cannot be constructed as a
/// provider at all.
///
/// The ten optional families are reached through the `as_*` accessors below.
/// Each defaults to `None`, so a minimal driver implements only what it
/// supports and inherits correct absence for everything else.
#[async_trait]
pub trait MemoryProvider: MemoryCore + MemoryRecall + MemoryPortability + 'static {
    /// Stable identifier for this driver (`tinycortex`, `supermemory`, `null`).
    ///
    /// Appears in status output, log lines, tracing spans, and audit events, so
    /// it must be stable across restarts and must not embed a URL, a token, or
    /// anything else user- or deployment-specific.
    fn driver_id(&self) -> &str;

    /// The families this driver implements.
    ///
    /// Asked **once** at bind time and cached: the kernel filters RPC
    /// registration and agent-tool assembly from the cached answer, so a set
    /// that changes after binding will not be noticed. A driver whose surface
    /// genuinely varies must report the union and answer
    /// [`MemoryError::Unsupported`] for the gaps.
    ///
    /// Must be honest: every advertised family must be reachable through its
    /// accessor. [`crate::provider::audit_provider`] checks exactly that.
    fn capabilities(&self) -> Capabilities;

    /// Current liveness, as the driver reports it.
    ///
    /// Called on bind and on demand for status output. Implementations should
    /// be cheap and must not block indefinitely — a health probe that hangs is
    /// indistinguishable from a subsystem that is down, but takes a timeout to
    /// find out.
    async fn health(&self) -> MemoryHealth;

    /// Release resources ahead of process exit or a rebind.
    ///
    /// Defaults to a successful no-op, because most drivers have nothing to
    /// release; a driver holding a connection pool or a background task should
    /// override it. The host's adapter forwards its `Driver::shutdown` here.
    ///
    /// Must be idempotent: a rebind followed by process exit calls it twice.
    ///
    /// # Errors
    ///
    /// Backend failures during teardown. The caller logs and continues —
    /// shutdown failure never blocks exit.
    async fn shutdown(&self) -> Result<(), MemoryError> {
        Ok(())
    }

    /// Bulk ingestion, when advertised.
    fn as_ingest(&self) -> Option<&dyn MemoryIngest> {
        None
    }

    /// The namespace-document tier, when advertised.
    fn as_documents(&self) -> Option<&dyn MemoryDocuments> {
        None
    }

    /// The summary tree, when advertised.
    fn as_tree(&self) -> Option<&dyn MemoryTree> {
        None
    }

    /// The entity index, when advertised.
    fn as_entities(&self) -> Option<&dyn MemoryEntities> {
        None
    }

    /// The key/value and relation graph, when advertised.
    fn as_graph(&self) -> Option<&dyn MemoryGraph> {
        None
    }

    /// Snapshot and change tracking, when advertised.
    fn as_diff(&self) -> Option<&dyn MemoryDiff> {
        None
    }

    /// The long-term goals document, when advertised.
    fn as_goals(&self) -> Option<&dyn MemoryGoals> {
        None
    }

    /// Per-tool learned rules, when advertised.
    fn as_tool_memory(&self) -> Option<&dyn MemoryToolMemory> {
        None
    }

    /// The host-sync write seam, when advertised.
    fn as_sources(&self) -> Option<&dyn MemorySourceSink> {
        None
    }

    /// Scheduler-driven upkeep, when advertised.
    fn as_maintenance(&self) -> Option<&dyn MemoryMaintenance> {
        None
    }

    /// Whether `capability` is actually **reachable** on this driver.
    ///
    /// This is the implementation-side truth, as opposed to
    /// [`Self::capabilities`], which is the advertised claim. The two should
    /// agree; [`crate::provider::audit_provider`] is where they are compared.
    ///
    /// The mandatory three are always `true` because they are supertraits. The
    /// remaining ten delegate to their accessor.
    ///
    /// The `match` is deliberately exhaustive: [`Capability`] is not
    /// `#[non_exhaustive]`, so adding a family without adding an accessor and
    /// an arm here is a compile error rather than a silent `false`.
    fn provides(&self, capability: Capability) -> bool {
        match capability {
            Capability::Core | Capability::Recall | Capability::Portability => true,
            Capability::Ingest => self.as_ingest().is_some(),
            Capability::Documents => self.as_documents().is_some(),
            Capability::Tree => self.as_tree().is_some(),
            Capability::Entities => self.as_entities().is_some(),
            Capability::Graph => self.as_graph().is_some(),
            Capability::Diff => self.as_diff().is_some(),
            Capability::Goals => self.as_goals().is_some(),
            Capability::ToolMemory => self.as_tool_memory().is_some(),
            Capability::Sources => self.as_sources().is_some(),
            Capability::Maintenance => self.as_maintenance().is_some(),
        }
    }
}
