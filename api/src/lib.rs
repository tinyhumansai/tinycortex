//! Stable public contracts for the TinyCortex memory system.
//!
//! This crate holds the value types, error enum, capability vocabulary, and
//! storage trait that both the `tinycortex` engine and its embedding hosts
//! compile against. It is deliberately dependency-light (serde / serde_json /
//! chrono / sha2 / anyhow / thiserror / async-trait / uuid only) so depending on
//! the contract never drags in SQLite, git2, reqwest, regex, or an async
//! runtime.
//!
//! ## Self-contained by design
//!
//! Nothing here names a host type. A third-party memory driver must be able to
//! depend on this crate alone, and the *generic* subsystem/driver vocabulary of
//! the OpenHuman kernel (`Driver`, `DriverClass`, `SubsystemRegistry`, the
//! policy `Guard`) must not be inherited from a *memory* crate by whichever
//! subsystem is cut over next. So the contract carries its own capability
//! vocabulary, and the host's memory adapter converts at the boundary.
//!
//! Driver *class* (embedded / external / null) is deliberately **absent**: that
//! is a host configuration fact about how a driver was bound, not something a
//! driver reports about itself.
//!
//! ## The engine's historical paths still resolve
//!
//! The engine crate aliases these modules back into their historical paths
//! (`tinycortex::memory::{types, error, traits}`,
//! `tinycortex::memory::chunks::types`, `tinycortex::memory::tree::runtime::types`,
//! `tinycortex::memory::tool_memory::types`, `tinycortex::memory::goals::types`),
//! so every existing path keeps resolving unchanged.
//!
//! ## Module map
//!
//! - [`types`]: pure data contracts (entries, hits, taint, namespaces).
//! - [`recall`]: the borrowed [`recall::RecallOpts`] and owned, serde-derived
//!   [`recall::OwnedRecallOpts`] recall filters (both re-exported from
//!   [`types`]).
//! - [`capabilities`]: the thirteen [`capabilities::Capability`] families and
//!   the [`capabilities::Capabilities`] set negotiated at bind time.
//! - [`version`]: [`CONTRACT_VERSION`] and the [`is_compatible`] bind rule.
//! - [`error`]: the typed [`error::MemoryError`] enum and its result alias.
//! - [`traits`]: the [`traits::Memory`] storage-backend trait.
//! - [`chunks`]: the persisted chunk model ([`chunks::Chunk`], [`chunks::Metadata`],
//!   [`chunks::SourceRef`], …) and the deterministic [`chunks::chunk_id`].
//! - [`tree`]: the markdown summary-tree node model ([`tree::TreeNode`],
//!   [`tree::NodeLevel`], [`tree::TreeStatus`], …).
//! - [`tool_memory`]: tool-scoped rule contracts ([`tool_memory::ToolMemoryRule`], …).
//! - [`goals`]: the long-term goals document ([`goals::GoalsDoc`], [`goals::GoalItem`]).

pub mod capabilities;
pub mod chunks;
pub mod error;
pub mod goals;
pub mod recall;
pub mod tool_memory;
pub mod traits;
pub mod tree;
pub mod types;
pub mod version;

pub use version::{is_compatible, CONTRACT_VERSION};
