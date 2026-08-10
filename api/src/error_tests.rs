//! Unit tests for [`super::MemoryError`], focused on the `Unsupported` variant
//! added for the driver contract. The older variants are exercised where they
//! are constructed, in the engine crate.

use super::*;
use crate::capabilities::Capability;

#[test]
fn unsupported_from_a_known_capability_uses_the_canonical_wire_name() {
    for capability in Capability::ALL {
        let err = MemoryError::unsupported(capability);
        match err {
            MemoryError::Unsupported {
                capability: ref got,
            } => {
                assert_eq!(got, capability.as_str());
            }
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }
}

#[test]
fn unsupported_raw_preserves_a_family_this_build_does_not_know() {
    // The reason the payload is an owned `String`: a driver speaking a newer
    // minor contract version can name a family that is not a `Capability` here,
    // and the adapter must be able to report it verbatim.
    let err = MemoryError::unsupported_raw("holographic_recall");
    match err {
        MemoryError::Unsupported { ref capability } => {
            assert_eq!(capability, "holographic_recall");
            assert!(Capability::parse(capability).is_err());
        }
        other => panic!("expected Unsupported, got {other:?}"),
    }
}

#[test]
fn unsupported_display_names_the_capability() {
    assert_eq!(
        MemoryError::unsupported(Capability::Tree).to_string(),
        "unsupported capability: tree"
    );
    assert_eq!(
        MemoryError::unsupported(Capability::ToolMemory).to_string(),
        "unsupported capability: tool_memory"
    );
}

#[test]
fn unsupported_is_distinguishable_from_the_other_variants() {
    // A transport adapter maps `501` to `Unsupported` and everything else
    // elsewhere, so the variant must not collide with `Invalid` / `NotFound`.
    let unsupported = MemoryError::unsupported(Capability::Diff);
    assert!(matches!(unsupported, MemoryError::Unsupported { .. }));

    let invalid = MemoryError::Invalid("diff".to_string());
    assert!(!matches!(invalid, MemoryError::Unsupported { .. }));
}
