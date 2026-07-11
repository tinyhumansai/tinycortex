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
//! - `providers-http`: compiles in `memory::providers` (reqwest-based
//!   embedding / LLM HTTP providers). Implies `tokio`.
//! - `rpc`: compiles in `memory::rpc` (the serde wire-envelope surface for
//!   exposing the engine over an RPC boundary).
//!
//! With every feature off, `cargo check` / `cargo test` still exercise the full
//! synchronous engine (storage, ingest, retrieval, tree, graph, goals, …); the
//! feature-gated modules only reserve a seam until their concrete
//! implementations land.

pub mod memory;
