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
//!
//! [`MemoryError::Unsupported`] is the one variant that belongs to the *driver
//! contract* rather than the engine: it is what a caller gets when a bound
//! driver does not implement the capability family a call needs. See its docs
//! for why that should be rare.

use thiserror::Error;

use crate::capabilities::Capability;

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
    /// The bound driver does not implement the named capability family.
    ///
    /// This should be **rare**, because capabilities are negotiated once at
    /// bind time and the kernel unregisters the RPC methods and omits the agent
    /// tools of every unadvertised family. Reaching this variant means one of:
    ///
    /// - an out-of-process driver answered `501` for a family its handshake
    ///   claimed (the case [`crate::capabilities`] cannot pre-empt);
    /// - a caller bypassed the capability filter — a kernel bug.
    ///
    /// ## Why the payload is an owned `String` and not a [`Capability`]
    ///
    /// The transport adapter constructs this from a wire response, where the
    /// family is a runtime string that may not be a known [`Capability`] at all
    /// — a driver speaking a newer minor contract version, a vendor extension,
    /// or simply a typo in a third-party backend. A `Capability` field would
    /// force the adapter to drop that information or fail parsing, and a
    /// `&'static str` cannot be produced from a runtime value without leaking
    /// memory. An owned `String` is the only representation that round-trips
    /// every case.
    ///
    /// Construct it with [`MemoryError::unsupported`] when the family is known
    /// (that path yields the canonical [`Capability::as_str`] spelling) and
    /// with [`MemoryError::unsupported_raw`] when it came off the wire.
    #[error("unsupported capability: {capability}")]
    Unsupported {
        /// Wire name of the capability family that is not supported —
        /// [`Capability::as_str`] when known, otherwise the raw string the
        /// driver reported.
        capability: String,
    },
    /// Catch-all wrapping an opaque lower-level error.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl MemoryError {
    /// Builds [`MemoryError::Unsupported`] for a family this build knows,
    /// using its canonical [`Capability::as_str`] spelling.
    pub fn unsupported(capability: Capability) -> Self {
        Self::Unsupported {
            capability: capability.as_str().to_string(),
        }
    }

    /// Builds [`MemoryError::Unsupported`] from a family name that came off the
    /// wire and may not correspond to any known [`Capability`].
    pub fn unsupported_raw(capability: impl Into<String>) -> Self {
        Self::Unsupported {
            capability: capability.into(),
        }
    }
}

/// Convenience result alias for engine-level fallible operations.
pub type MemoryEngineResult<T> = Result<T, MemoryError>;

#[cfg(test)]
#[path = "error_tests.rs"]
mod tests;
