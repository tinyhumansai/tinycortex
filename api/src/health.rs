//! Liveness state a memory driver reports about itself.
//!
//! ## Why this lives in the contract crate and not in the host
//!
//! The OpenHuman kernel has (or will have) a *generic* subsystem-agnostic
//! `DriverHealth` shared by memory, inference, channels, and sandbox. This crate
//! cannot name that type: `tinycortex-api` is the contract a third-party driver
//! compiles against, and a driver must be able to depend on it without pulling
//! in the OpenHuman host — nor should the next subsystem cut over inherit
//! generic kernel vocabulary from a *memory* crate.
//!
//! So the contract carries its own [`MemoryHealth`], and the host's memory
//! adapter converts. The conversion is deliberately trivial and lossless: this
//! is a **small closed enum with a reason string**, shaped one-for-one against
//! the kernel's `Ready | Degraded { reason } | Down { reason }`, not a
//! free-form struct that would need field-by-field mapping and would drift.
//! Keep it that way — if a driver needs to report something richer, it belongs
//! in a driver-specific status payload, not here.
//!
//! ## Wire form
//!
//! Serializes as an internally-tagged object with a stable snake_case `status`
//! discriminant, which is also the shape of the transport adapter's
//! `GET /v1/health` → `{ status, detail }` response:
//!
//! ```json
//! { "status": "ready" }
//! { "status": "degraded", "reason": "vector index rebuilding" }
//! { "status": "down", "reason": "connection refused" }
//! ```

use serde::{Deserialize, Serialize};

/// Health of a bound memory driver, as the driver reports it.
///
/// The three states are ordered by severity and mean different things to the
/// kernel:
///
/// - [`MemoryHealth::Ready`] — serve traffic normally.
/// - [`MemoryHealth::Degraded`] — still serve traffic, but surface the reason
///   in status output; results may be incomplete or slow.
/// - [`MemoryHealth::Down`] — do not serve traffic; the bind should be surfaced
///   as failed and, per the fallback rule, the embedded default rebound.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum MemoryHealth {
    /// The driver is reachable and serving requests normally.
    Ready,
    /// The driver is serving requests, but something is wrong and the caller
    /// should surface it. Results may be incomplete, stale, or slow.
    Degraded {
        /// Operator-facing explanation. Must not contain credentials, tokens,
        /// or user memory content — this string is logged and shown in status
        /// output.
        reason: String,
    },
    /// The driver cannot serve requests at all.
    Down {
        /// Operator-facing explanation, subject to the same redaction rule as
        /// [`MemoryHealth::Degraded::reason`].
        reason: String,
    },
}

impl MemoryHealth {
    /// Convenience constructor for [`MemoryHealth::Degraded`].
    pub fn degraded(reason: impl Into<String>) -> Self {
        Self::Degraded {
            reason: reason.into(),
        }
    }

    /// Convenience constructor for [`MemoryHealth::Down`].
    pub fn down(reason: impl Into<String>) -> Self {
        Self::Down {
            reason: reason.into(),
        }
    }

    /// Stable snake_case discriminant, matching the serialized `status` field.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Degraded { .. } => "degraded",
            Self::Down { .. } => "down",
        }
    }

    /// The operator-facing reason, when there is one. `None` for
    /// [`MemoryHealth::Ready`].
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Ready => None,
            Self::Degraded { reason } | Self::Down { reason } => Some(reason.as_str()),
        }
    }

    /// Whether the kernel should route traffic to this driver.
    ///
    /// True for [`MemoryHealth::Ready`] and [`MemoryHealth::Degraded`] — a
    /// degraded driver is still the bound driver — and false for
    /// [`MemoryHealth::Down`].
    pub fn is_usable(&self) -> bool {
        !matches!(self, Self::Down { .. })
    }
}

impl std::fmt::Display for MemoryHealth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.reason() {
            Some(reason) => write!(f, "{}: {reason}", self.as_str()),
            None => f.write_str(self.as_str()),
        }
    }
}

#[cfg(test)]
#[path = "health_tests.rs"]
mod tests;
