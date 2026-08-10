//! The memory driver contract: [`MemoryProvider`] plus the thirteen capability
//! family traits a driver may implement.
//!
//! ## Shape
//!
//! ```text
//! MemoryProvider  ── identity, capabilities, health, shutdown
//!   : MemoryCore          (mandatory — supertrait, always callable)
//!   : MemoryRecall        (mandatory — supertrait, always callable)
//!   : MemoryPortability   (mandatory — supertrait, always callable)
//!   ├─ as_ingest()       -> Option<&dyn MemoryIngest>
//!   ├─ as_documents()    -> Option<&dyn MemoryDocuments>
//!   ├─ as_tree()         -> Option<&dyn MemoryTree>
//!   ├─ as_entities()     -> Option<&dyn MemoryEntities>
//!   ├─ as_graph()        -> Option<&dyn MemoryGraph>
//!   ├─ as_diff()         -> Option<&dyn MemoryDiff>
//!   ├─ as_goals()        -> Option<&dyn MemoryGoals>
//!   ├─ as_tool_memory()  -> Option<&dyn MemoryToolMemory>
//!   ├─ as_sources()      -> Option<&dyn MemorySourceSink>
//!   └─ as_maintenance()  -> Option<&dyn MemoryMaintenance>
//! ```
//!
//! The mandatory three are supertraits, so "mandatory" is enforced by the type
//! system rather than by a runtime check. The optional ten are accessors that
//! default to `None`, so absence is the default and presence is opt-in.
//!
//! ## Rules that bind every family
//!
//! 1. **Typed errors, always.** Every method returns
//!    `Result<_, MemoryError>`. The transport adapter maps an out-of-process
//!    `501` onto [`crate::error::MemoryError::Unsupported`], and the kernel
//!    distinguishes "cannot" from "failed". `anyhow::Error` would erase that.
//! 2. **No configuration crosses the boundary.** Not one signature names a
//!    config type. A driver holds its own configuration; the contract passes
//!    domain arguments only.
//! 3. **No host types.** Nothing here names an OpenHuman type, so a
//!    third-party driver depends on this crate alone.
//! 4. **The driver never assigns provenance.** [`crate::types::MemoryTaint`] is
//!    an argument on every write path and a preserved field on every import.
//! 5. **The host owns the loop.** Sealing, cascading, maintenance, and source
//!    sync are all "run one step when asked"; no driver installs a background
//!    task or hooks the agent turn.
//! 6. **Object safety throughout.** No generics, no `Self` returns, no
//!    associated constants — every family is usable as `&dyn`.
//!
//! ## Reference implementation
//!
//! [`crate::null::NullMemoryProvider`] implements all thirteen families:
//! `/dev/null` semantics for the mandatory three, and
//! [`crate::error::MemoryError::Unsupported`] for the other ten, which it does
//! not advertise. It is what a compiled-out or unconfigured memory subsystem
//! binds to, and it doubles as the proof that the mandatory set is
//! implementable without a storage engine.

pub mod audit;
pub mod content;
pub mod driver;
pub mod knowledge;
pub mod mandatory;
pub mod records;
pub mod types;

pub use audit::{audit_provider, CapabilityAudit};
pub use content::{MemoryDocuments, MemoryIngest, MemoryTree};
pub use driver::MemoryProvider;
pub use knowledge::{MemoryDiff, MemoryEntities, MemoryGraph};
pub use mandatory::{MemoryCore, MemoryPortability, MemoryRecall};
pub use records::{MemoryGoals, MemoryMaintenance, MemorySourceSink, MemoryToolMemory};
pub use types::{
    ChangeKind, DiffReport, EntityHit, EntityRef, ExportPage, ExportRecord, ImportOutcome,
    IngestItem, IngestOutcome, MaintenanceReport, SnapshotRef, SourceChange, SourceItem,
    SourceScope,
};
