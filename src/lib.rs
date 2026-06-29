//! Rust core for TinyCortex.
//!
//! TinyCortex is the memory engine extracted from OpenHuman as a standalone,
//! config-driven, test-driven library. OpenHuman (or any other host) imports
//! this crate to ingest source-scoped payloads, canonicalize and chunk them,
//! score and embed them, build summary trees, and retrieve explainable context.
//!
//! The public surface lives under [`memory`]. See `docs/openhuman-memory-engine-spec.md`
//! for the functional specification and ownership boundaries.

pub mod memory;
