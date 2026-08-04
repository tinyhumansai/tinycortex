//! Unit tests for the contract version rule in [`super`].
//!
//! The rule these pin is the one from the kernel design: a **minor** bump means
//! a capability was added and stays compatible; a **major** mismatch refuses
//! the bind.

use super::*;

#[test]
fn contract_version_starts_at_one_zero() {
    assert_eq!(CONTRACT_VERSION, (1, 0));
}

#[test]
fn own_version_is_compatible_with_itself() {
    assert!(is_compatible(CONTRACT_VERSION));
}

#[test]
fn a_minor_bump_stays_compatible_in_both_directions() {
    let (major, minor) = CONTRACT_VERSION;

    // Remote ahead: it advertises families this build does not know. Unknown
    // family strings are skipped during handshake parsing.
    assert!(is_compatible((major, minor + 1)));
    assert!(is_compatible((major, minor + 25)));
    assert!(is_compatible((major, u16::MAX)));

    // Remote behind: it lacks families this build knows. Those simply are not
    // advertised, so the surface degrades — the ordinary path, not an error.
    assert!(is_compatible((major, minor.saturating_sub(1))));
    assert!(is_compatible((major, 0)));
}

#[test]
fn a_major_mismatch_refuses_the_bind() {
    let (major, minor) = CONTRACT_VERSION;

    // Remote ahead by a major: an existing signature changed under us.
    assert!(!is_compatible((major + 1, 0)));
    assert!(!is_compatible((major + 1, minor)));
    assert!(!is_compatible((major + 1, u16::MAX)));

    // Remote behind by a major: same reasoning, other direction. A newer minor
    // does not rescue an older major.
    assert!(!is_compatible((major - 1, u16::MAX)));
    assert!(!is_compatible((0, 0)));
}

#[test]
fn adding_a_method_to_an_already_advertised_family_requires_a_major_bump() {
    // Capability negotiation has family granularity, not method granularity:
    // there is no way to advertise "Core, but without the new method". So a
    // method added to a family a driver may already advertise (e.g. Core,
    // Recall) cannot be made minor-safe by negotiation the way a brand-new
    // capability family can — an older driver still advertising that family
    // would be called into a method it never implemented. This is why the
    // module docs classify that addition as a MAJOR bump, not minor, even
    // though it looks additive. This test exists so the rule cannot be
    // re-derived from `is_compatible`'s code alone, which only encodes "major
    // halves must match" and says nothing about *why* a same-family method
    // addition belongs on the major side of that line.
    assert!(
        !is_compatible((CONTRACT_VERSION.0 + 1, 0)),
        "a method added to an existing family must ship as a major bump, \
         which this asserts refuses the bind against an old build"
    );
}

#[test]
fn compatibility_depends_only_on_the_major_half() {
    let (major, _) = CONTRACT_VERSION;
    for minor in [0u16, 1, 2, 7, 999, u16::MAX] {
        assert!(
            is_compatible((major, minor)),
            "minor {minor} should not affect compatibility"
        );
        assert!(
            !is_compatible((major + 1, minor)),
            "minor {minor} must not rescue a major mismatch"
        );
    }
}
