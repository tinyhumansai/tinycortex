//! Stable public contracts for the TinyCortex memory system.
//!
//! # This crate is now a re-export
//!
//! Every item here comes from [`tinymemory_api`], which was extracted from this
//! crate and is its single source of truth (tinymemory#18 §A1). The paths are
//! unchanged and the types are the same types, so an existing dependant needs
//! no edit; what goes away is the second, nominally-distinct copy that a
//! conversion layer had to keep in step by hand.
//!
//! This crate *names* the value types, error enum, capability vocabulary, and
//! storage trait that both the `tinycortex` engine and its embedding hosts
//! compile against — it no longer defines them. Its one dependency is
//! `tinymemory-api`, which holds the same "no SQLite, no git2, no reqwest, no
//! regex, no async runtime" rule and enforces it in its own CI, so depending on
//! this crate still drags none of them in.
//!
//! ## Self-contained by design
//!
//! Nothing here names a host type. A third-party memory driver must be able to
//! depend on this crate alone, and the *generic* subsystem/driver vocabulary of
//! the OpenHuman kernel (`Driver`, `DriverClass`, `SubsystemRegistry`, the
//! policy `Guard`) must not be inherited from a *memory* crate by whichever
//! subsystem is cut over next. So the contract carries its own identity,
//! capability, and health vocabulary, and the host's memory adapter converts at
//! the boundary — see [`health`] for the shape that conversion relies on.
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
//! - [`capabilities`]: the eighteen [`capabilities::Capability`] families and
//!   the [`capabilities::Capabilities`] set negotiated at bind time. Thirteen
//!   before the re-export; the extra five come with the contract.
//! - [`provider`]: the driver contract — [`provider::MemoryProvider`] plus the
//!   capability family traits and the value types they need.
//! - [`null`]: [`null::NullMemoryProvider`], the reference driver a
//!   compiled-out or unconfigured memory subsystem binds to.
//! - [`health`]: [`health::MemoryHealth`], the liveness state a driver reports.
//! - [`version`]: [`CONTRACT_VERSION`] and the [`is_compatible`] bind rule.
//! - [`error`]: the typed [`error::MemoryError`] enum and its result alias.
//! - [`traits`]: the [`traits::Memory`] storage-backend trait.
//! - [`chunks`]: the persisted chunk model ([`chunks::Chunk`], [`chunks::Metadata`],
//!   [`chunks::SourceRef`], …) and the deterministic [`chunks::chunk_id`].
//! - [`tree`]: the markdown summary-tree node model ([`tree::TreeNode`],
//!   [`tree::NodeLevel`], [`tree::TreeStatus`], …).
//! - [`tool_memory`]: tool-scoped rule contracts ([`tool_memory::ToolMemoryRule`], …).
//! - [`goals`]: the long-term goals document ([`goals::GoalsDoc`], [`goals::GoalItem`]).

// Every module below is `tinymemory-api`'s, re-exported. This crate defined its
// own copies until tinymemory#18 §A1; they were the same types by construction —
// `tinymemory-api` was extracted from this crate and held byte-identical — but
// being *nominally* distinct meant `adapters/tinycortex/src/convert.rs` had to
// translate between them on every call, and a field added to one had to be
// added to the other and to the conversion, in three places, or a value was
// silently dropped.
//
// Re-exporting keeps every existing path working: `tinycortex_api::types::
// MemoryEntry` still resolves, and now resolves to the same type the contract
// names. Nothing downstream has to be rewritten to gain that.
//
// Two things the re-export changes on purpose. The capability vocabulary grows
// from thirteen families to eighteen — the engine matches on none of them, so
// nothing here is affected — and `CONTRACT_VERSION` becomes the contract's own
// `(2, 2)` rather than this crate's stale `(1, 0)`. That version was already
// wrong: hosts bind against the contract, and this crate has not been the thing
// they compile against for some time. The engine never reads it.
pub use tinymemory_api::{
    capabilities, chunks, error, goals, health, null, provider, recall, tool_memory, traits, tree,
    types, version,
};

pub use tinymemory_api::{is_compatible, CONTRACT_VERSION};
