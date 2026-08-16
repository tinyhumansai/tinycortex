//! Rust core for TinyCortex.
//!
//! TinyCortex is the memory engine extracted from OpenHuman as a standalone,
//! config-driven, test-driven library. OpenHuman (or any other host) imports
//! this crate to ingest source-scoped payloads, canonicalize and chunk them,
//! score and embed them, build summary trees, and retrieve explainable context.
//!
//! The public surface lives under [`memory`]. See `docs/openhuman-memory-engine-spec.md`
//! for the functional specification and ownership boundaries.
//!
//! ## Feature flags
//!
//! The crate defaults to a fully synchronous, dependency-light core (`default = []`
//! in `Cargo.toml`). Heavier capabilities are opt-in via Cargo features so a host
//! that only needs the synchronous engine never links async runtimes, native git
//! bindings, or an HTTP client:
//!
//! - `tokio`: enables always-on async background loops for the job queue
//!   (`memory::queue::runtime`) instead of driving `run_once` by hand.
//! - `git-diff`: compiles in `memory::diff` (git-backed source snapshots,
//!   diffs, checkpoints, read markers) and its native `git2`/libgit2 dependency.
//! - `sync`: compiles in the live Composio and workspace synchronization
//!   pipelines and implies `tokio`.
//!
//! With every feature off, `cargo check` / `cargo test` still exercise the full
//! synchronous engine (storage, ingest, retrieval, tree, graph, goals, …); the
//! feature-gated modules contain concrete optional implementations.

pub mod memory;

/// The `git2` binding this crate links, re-exported for hosts.
///
/// This crate owns every use of libgit2 in the memory stack — the diff ledger
/// (`memory::diff`), the wiki mirror (`memory::store::content::wiki_git`), and
/// the persona git-history reader. A host therefore has no reason to declare
/// `git2` itself, and one that did would risk pinning a different major and
/// linking libgit2 twice (`links = "git2"` makes that a hard cargo error).
///
/// Reach for this only where a caller genuinely has to inspect a ledger or
/// mirror this crate produced — an integration test asserting on commits, tags
/// or trees. Ordinary callers should stay on the typed API above.
#[cfg(any(feature = "git-diff", feature = "wiki-git"))]
pub use git2;
