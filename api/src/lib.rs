//! Stable public contracts for the TinyCortex memory system.
//!
//! This crate holds the value types, error enum, and storage trait that both
//! the `tinycortex` engine and its embedding hosts compile against. It is
//! deliberately dependency-light (serde / serde_json / chrono / sha2 / anyhow /
//! thiserror / async-trait only) so depending on the contract never drags in
//! SQLite, git2, reqwest, regex, or an async runtime.
//!
//! The engine crate aliases these modules back into their historical paths
//! (`tinycortex::memory::{types, error, traits}`,
//! `tinycortex::memory::chunks::types`, `tinycortex::memory::tree::runtime::types`,
//! `tinycortex::memory::tool_memory::types`, `tinycortex::memory::goals::types`),
//! so every existing path keeps resolving unchanged.
//!
//! - [`types`]: pure data contracts (entries, hits, taint, namespaces).
//! - [`error`]: the typed [`error::MemoryError`] enum and its result alias.
//! - [`traits`]: the [`traits::Memory`] storage-backend trait.
//! - [`chunks`]: the persisted chunk model ([`chunks::Chunk`], [`chunks::Metadata`],
//!   [`chunks::SourceRef`], …) and the deterministic [`chunks::chunk_id`].
//! - [`tree`]: the markdown summary-tree node model ([`tree::TreeNode`],
//!   [`tree::NodeLevel`], [`tree::TreeStatus`], …).
//! - [`tool_memory`]: tool-scoped rule contracts ([`tool_memory::ToolMemoryRule`], …).
//! - [`goals`]: the long-term goals document ([`goals::GoalsDoc`], [`goals::GoalItem`]).

pub mod chunks;
pub mod error;
pub mod goals;
pub mod tool_memory;
pub mod traits;
pub mod tree;
pub mod types;
