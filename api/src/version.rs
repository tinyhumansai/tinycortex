//! The memory contract version and the compatibility rule that governs it.
//!
//! Re-exported at the crate root, so the canonical paths are
//! [`crate::CONTRACT_VERSION`] and [`crate::is_compatible`].
//!
//! ## The rule
//!
//! `CONTRACT_VERSION` is `(major, minor)`:
//!
//! - **Minor bump — a capability was added.** A new [`crate::capabilities::Capability`]
//!   family, or a new method inside an existing family that older drivers may
//!   simply not implement. Nothing that already compiled stops compiling.
//! - **Major bump — an existing signature changed.** A method's parameters or
//!   return type moved, a mandatory family was added or removed, or a wire
//!   string changed. Anything an existing driver already implements now means
//!   something different.
//!
//! ## Why only the major half gates the bind
//!
//! An out-of-process driver reports the version it speaks in its handshake
//! (`POST /v1/handshake` → `{ contract_version, driver_id, capabilities[] }`).
//! **A major mismatch refuses the bind**; a minor difference in either
//! direction is accepted, because capability negotiation already covers it:
//!
//! - remote minor > local minor — the driver advertises families this build has
//!   never heard of. Unknown family strings are skipped during handshake
//!   parsing, so this kernel simply never calls them.
//! - remote minor < local minor — the driver is missing families this build
//!   knows about. It does not advertise them, so the corresponding RPC methods
//!   are unregistered and the agent tools are absent. That is the ordinary
//!   degradation path, not an error.
//!
//! Refusing on a minor difference would therefore reject a driver that is
//! perfectly usable, and would make adding a family a fleet-wide breaking
//! change — which is exactly what the major/minor split exists to avoid.
//!
//! Encoding the rule here rather than in prose means a caller cannot get it
//! subtly wrong: the bind path calls [`is_compatible`], never compares tuples
//! by hand.

/// Version of the memory contract this crate defines, as `(major, minor)`.
///
/// See the module docs for the bump rule. Bump the **minor** half when a
/// capability family or an additive method lands; bump the **major** half — and
/// reset the minor to `0` — when an existing signature, mandatory family, or
/// wire string changes.
pub const CONTRACT_VERSION: (u16, u16) = (1, 0);

/// Whether a driver speaking `remote` can be bound against this build.
///
/// Compatible exactly when the major halves match. See the module docs for why
/// the minor half is informational.
///
/// # Examples
///
/// ```
/// use tinycortex_api::{is_compatible, CONTRACT_VERSION};
///
/// // The version this build speaks is always compatible with itself.
/// assert!(is_compatible(CONTRACT_VERSION));
///
/// // A minor difference in either direction is fine — capability negotiation
/// // covers the delta.
/// assert!(is_compatible((CONTRACT_VERSION.0, CONTRACT_VERSION.1 + 7)));
///
/// // A major mismatch refuses the bind.
/// assert!(!is_compatible((CONTRACT_VERSION.0 + 1, 0)));
/// ```
pub fn is_compatible(remote: (u16, u16)) -> bool {
    remote.0 == CONTRACT_VERSION.0
}

#[cfg(test)]
#[path = "version_tests.rs"]
mod tests;
