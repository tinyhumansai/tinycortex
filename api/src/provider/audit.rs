//! The honesty check: does a driver's advertised capability set match the
//! surface it actually exposes?
//!
//! [`MemoryProvider::capabilities`] is a *claim*, and the kernel acts on it —
//! it registers RPC methods and assembles agent tools from the advertised set
//! and never re-checks. A driver that advertises a family it does not implement
//! therefore produces a surface that exists in `/schema`, appears in the agent's
//! tool list, and fails on first use. That is precisely the
//! "registered-but-failing" outcome the degradation design exists to avoid.
//!
//! [`audit_provider`] compares the claim against
//! [`MemoryProvider::provides`] — which is derived from the accessors, so it
//! cannot drift from reality — and reports both directions of mismatch. Run it
//! at bind time next to [`crate::capabilities::Capabilities::validate`], and in
//! every driver's own test suite.
//!
//! The two directions mean different things:
//!
//! - **Advertised but absent** is a bug that will surface as a failing call. It
//!   should refuse the bind.
//! - **Present but unadvertised** is dead surface: the family works but the
//!   kernel unregistered it, so nothing can reach it. Usually a forgotten
//!   entry in the driver's `capabilities()` list.

use std::fmt;

use crate::capabilities::Capability;
use crate::error::MemoryError;
use crate::provider::driver::MemoryProvider;

/// A disagreement between what a driver advertises and what it implements.
///
/// Carries the families structurally rather than as a formatted string so a
/// caller can report them in a status payload or a bind-failure event as well
/// as in a log line. At least one of the two vectors is non-empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityAudit {
    /// Families the driver advertises but does not expose. These will fail on
    /// first call; refuse the bind.
    pub advertised_but_absent: Vec<Capability>,
    /// Families the driver exposes but does not advertise. These are
    /// unreachable, because the kernel filters from the advertised set.
    pub present_but_unadvertised: Vec<Capability>,
}

impl fmt::Display for CapabilityAudit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts = Vec::new();
        if !self.advertised_but_absent.is_empty() {
            parts.push(format!(
                "advertised but not implemented: {}",
                join(&self.advertised_but_absent)
            ));
        }
        if !self.present_but_unadvertised.is_empty() {
            parts.push(format!(
                "implemented but not advertised: {}",
                join(&self.present_but_unadvertised)
            ));
        }
        write!(f, "memory driver capability mismatch; {}", parts.join("; "))
    }
}

impl std::error::Error for CapabilityAudit {}

impl From<CapabilityAudit> for MemoryError {
    /// A mismatch is the driver saying something untrue about itself, which is
    /// a configuration/implementation error rather than an unsupported call —
    /// hence [`MemoryError::Invalid`] and not
    /// [`MemoryError::Unsupported`]. Same reasoning as
    /// [`crate::capabilities::MissingMandatoryCapabilities`].
    fn from(value: CapabilityAudit) -> Self {
        MemoryError::Invalid(value.to_string())
    }
}

fn join(families: &[Capability]) -> String {
    families
        .iter()
        .map(|cap| cap.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Compare a driver's advertised capability set against its reachable surface.
///
/// Walks every [`Capability`] in declaration order, so the returned vectors are
/// in that order too.
///
/// # Errors
///
/// Returns [`CapabilityAudit`] when the two disagree in either direction. A
/// driver that agrees with itself returns `Ok(())`.
///
/// # Examples
///
/// ```
/// # use tinycortex_api::null::NullMemoryProvider;
/// # use tinycortex_api::provider::audit_provider;
/// // The reference null driver is self-consistent.
/// assert!(audit_provider(&NullMemoryProvider::new()).is_ok());
/// ```
pub fn audit_provider(provider: &dyn MemoryProvider) -> Result<(), CapabilityAudit> {
    let advertised = provider.capabilities();
    let mut advertised_but_absent = Vec::new();
    let mut present_but_unadvertised = Vec::new();

    for capability in Capability::ALL {
        match (
            advertised.contains(capability),
            provider.provides(capability),
        ) {
            (true, false) => advertised_but_absent.push(capability),
            (false, true) => present_but_unadvertised.push(capability),
            _ => {}
        }
    }

    if advertised_but_absent.is_empty() && present_but_unadvertised.is_empty() {
        Ok(())
    } else {
        Err(CapabilityAudit {
            advertised_but_absent,
            present_but_unadvertised,
        })
    }
}

#[cfg(test)]
#[path = "audit_tests.rs"]
mod tests;
