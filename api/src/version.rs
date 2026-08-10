//! The memory contract version and the compatibility rule that governs it.
//!
//! Re-exported at the crate root, so the canonical paths are
//! [`crate::CONTRACT_VERSION`] and [`crate::is_compatible`].
//!
//! ## The rule
//!
//! `CONTRACT_VERSION` is `(major, minor)`:
//!
//! - **Minor bump — an addition that capability negotiation alone makes safe.**
//!   A new [`crate::capabilities::Capability`] family is the canonical case: an
//!   older driver simply never advertises it, the corresponding RPC methods are
//!   unregistered, and the kernel never calls in. A new optional field on an
//!   existing wire type, or a new error variant an older kernel can treat as
//!   opaque, are the same shape — nothing that already compiled stops
//!   compiling, and there is no way for an old driver to be asked for
//!   something it never claimed to support.
//! - **Major bump — an existing signature changed, OR a method was added to an
//!   already-advertised family.** A method's parameters or return type moved, a
//!   mandatory family was added or removed, a wire string changed — or a driver
//!   advertising an existing family (say [`crate::capabilities::Capability::Core`])
//!   now has to implement one more method on it. That last case looks additive
//!   but is not: capability negotiation has **family granularity only** — there
//!   is no way to advertise "`Core`, but without the new method" — so an older
//!   driver that still advertises `Core` can be called into a method it does
//!   not implement. Bump the major half instead, which forces every driver
//!   claiming that family to actually implement the new surface before it can
//!   bind again.
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
/// See the module docs for the bump rule. Bump the **minor** half only for an
/// addition capability negotiation alone makes safe — a new capability family,
/// a new optional wire field, a new opaque-to-old-kernels error variant. Bump
/// the **major** half — and reset the minor to `0` — for an existing signature
/// change, a mandatory family change, a wire string change, **or a new method
/// added to a family a driver may already advertise** (negotiation is
/// family-granular, not method-granular, so that case cannot be made minor-safe
/// by negotiation alone).
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
