//! Stable public contracts for the TinyCortex memory system.
//!
//! This crate holds the value types, error enum, and storage trait that both
//! the `tinycortex` engine and its embedding hosts compile against. It is
//! deliberately dependency-light (serde / anyhow / thiserror / async-trait
//! only) so depending on the contract never drags in SQLite, git2, reqwest, or
//! an async runtime.
//!
//! The engine crate re-exports these modules as `tinycortex::memory::{types,
//! error, traits}`, so existing paths keep resolving unchanged.
//!
//! - [`types`]: pure data contracts (entries, hits, taint, namespaces).
//! - [`error`]: the typed [`error::MemoryError`] enum and its result alias.
//! - [`traits`]: the [`traits::Memory`] storage-backend trait.

pub mod error;
pub mod traits;
pub mod types;
