//! Retrieval-effectiveness benchmark harness for TinyCortex memory backends.
//!
//! This crate measures *how good* retrieval is (recall@k, precision@k, MRR,
//! nDCG@k) — a complement to correctness tests, which only prove it doesn't
//! error, and to `cargo bench`, which measures speed. It is deliberately small
//! and backend-agnostic: a [`backend::BenchBackend`] implementation is the only
//! seam, so the same harness scores today's lexical [`InMemoryMemoryStore`]
//! ([`backend::InMemoryBackend`]) and, later, an assembled `CortexEngine` or a
//! live-embedding backend.
//!
//! Pipeline: load a labeled [`dataset::Dataset`] → ingest into a backend → run
//! every query → aggregate [`metrics`] into a [`harness::RunReport`] (serialized
//! to dated JSON by the `effectiveness` binary for cross-commit diffs).
//!
//! Corresponds to goal T3 in `docs/plan/03-testing-benchmarks.md`.

pub mod backend;
pub mod dataset;
pub mod harness;
pub mod metrics;
