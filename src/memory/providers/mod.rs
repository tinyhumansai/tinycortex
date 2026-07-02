//! HTTP-backed embedding / LLM providers (feature `providers-http`).
//!
//! This module is the seam for reqwest-based provider implementations that back
//! the scoring and embedding signals with real network services. The concrete
//! providers (and their wire contracts) land with goals **C3** (embedding
//! providers) and **M3** (LLM providers); this module reserves the boundary and
//! gates the heavy [`reqwest`] dependency behind the `providers-http` feature so
//! the dependency-light core never links it.
//!
//! ## Embedding signature invariant
//!
//! Any embedding provider added here MUST report its identity as the canonical
//! signature string `provider=<name>;model=<model_id>;dims=<dims>` so that
//! vectors stay comparable across the store, retrieval, and re-embed backfill
//! paths. See `docs/plan/01-migration-library.md` §1.3.

/// Shared async HTTP client type for provider implementations.
///
/// Aliased here so the reqwest dependency stays confined to this feature-gated
/// module and provider code has a single, stable client type to build against.
pub type HttpClient = reqwest::Client;
