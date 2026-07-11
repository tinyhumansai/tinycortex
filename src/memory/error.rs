//! Engine-level error type shared by ported modules that want a typed error
//! surface. Modules that mirror OpenHuman's `anyhow`-based signatures may keep
//! using `anyhow::Result`; this enum is for contracts that benefit from
//! matchable variants (validation, not-found, taint, IO).
//!
//! `?` converts `std::io::Error` and `serde_json::Error` into
//! [`MemoryError::Io`] / [`MemoryError::Serde`] automatically via the derived
//! `#[from]` impls, and any `anyhow::Error` (including one produced by `?` on
//! a foreign error type inside an `anyhow`-returning function) into
//! [`MemoryError::Other`]. The purpose-built variants ([`MemoryError::NotFound`],
//! [`MemoryError::Invalid`], [`MemoryError::BudgetExceeded`],
//! [`MemoryError::PathEscape`]) are constructed explicitly by callers that want
//! matchable, typed failure — they are never inferred from a foreign error.

use thiserror::Error;

/// Errors surfaced by the memory engine.
#[derive(Debug, Error)]
pub enum MemoryError {
    /// A requested record / source / node was not found.
    #[error("not found: {0}")]
    NotFound(String),
    /// Caller-supplied input failed validation.
    #[error("invalid input: {0}")]
    Invalid(String),
    /// A configured budget (tokens, cost, depth) was exceeded.
    #[error("budget exceeded: {0}")]
    BudgetExceeded(String),
    /// A path escaped the workspace sandbox (symlink / traversal).
    #[error("path escapes workspace: {0}")]
    PathEscape(String),
    /// Underlying IO failure.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// Serialization / deserialization failure.
    #[error("serde error: {0}")]
    Serde(#[from] serde_json::Error),
    /// Catch-all wrapping an opaque lower-level error.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Convenience result alias for engine-level fallible operations.
pub type MemoryEngineResult<T> = Result<T, MemoryError>;
