//! serde schema / envelope surface for the RPC boundary (feature `rpc`).
//!
//! This module is the seam for the wire-facing request/response envelope and
//! schema types that expose the memory engine over an RPC boundary. The
//! concrete envelope contract lands with goal **C5**; this module reserves the
//! surface and gates it behind the `rpc` feature so hosts that only embed the
//! engine in-process never compile the wire layer.
//!
//! No heavy dependencies are pulled in: the RPC surface is built on the core
//! `serde` / `serde_json` stack that the engine already depends on.
//!
//! Wire-format invariants that this surface MUST preserve when populated (see
//! `docs/plan/01-migration-library.md` §1.3): embedding signatures, archivist
//! leaf ids, and the `MemoryTaint` `internal` / `external_sync` strings that
//! fail **closed** to `external_sync`.
