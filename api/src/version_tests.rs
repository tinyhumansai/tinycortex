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
