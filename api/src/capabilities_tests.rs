//! Unit tests for the capability vocabulary in [`super`].
//!
//! Three properties are load-bearing and each has its own test:
//!
//! 1. the enum has exactly the thirteen contract families and no more;
//! 2. the serialized form is stable snake_case **strings**, never discriminant
//!    integers — a driver deployed against an older build must keep advertising
//!    the same set after a variant is inserted mid-enum;
//! 3. [`super::Capabilities::validate`] rejects a set missing **any** of the
//!    three mandatory families, checked one family at a time.

use super::*;
use serde_json::json;

#[test]
fn capability_has_exactly_the_thirteen_contract_families() {
    assert_eq!(Capability::ALL.len(), 13);
    assert_eq!(Capability::all().len(), 13);

    let names: Vec<&str> = Capability::ALL.iter().map(|c| c.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "core",
            "recall",
            "ingest",
            "documents",
            "tree",
            "entities",
            "graph",
            "diff",
            "goals",
            "tool_memory",
            "sources",
            "maintenance",
            "portability",
        ]
    );
}

#[test]
fn capability_all_has_no_duplicates() {
    let mut seen = std::collections::BTreeSet::new();
    for capability in Capability::ALL {
        assert!(
            seen.insert(capability.as_str()),
            "duplicate capability in ALL: {capability}"
        );
    }
}

#[test]
fn capability_as_str_matches_serde_representation() {
    // The wire form is the stable contract; `as_str` is the authority and the
    // derive must agree with it for every variant.
    for capability in Capability::ALL {
        assert_eq!(
            serde_json::to_value(capability).unwrap(),
            json!(capability.as_str()),
            "serde form drifted from as_str for {capability}"
        );
    }
}

#[test]
fn capability_serializes_as_a_string_not_an_integer() {
    // Guards the specific regression the string form exists to prevent:
    // inserting a variant must not re-map an already-deployed driver's set.
    for capability in Capability::ALL {
        assert!(
            serde_json::to_value(capability).unwrap().is_string(),
            "{capability} did not serialize as a string"
        );
    }
}

#[test]
fn capability_parse_round_trips_every_variant() {
    for capability in Capability::ALL {
        assert_eq!(Capability::parse(capability.as_str()), Ok(capability));
        assert_eq!(
            capability.as_str().parse::<Capability>(),
            Ok(capability),
            "FromStr disagreed with parse for {capability}"
        );
        let decoded: Capability =
            serde_json::from_value(json!(capability.as_str())).expect("known family decodes");
        assert_eq!(decoded, capability);
    }
}

#[test]
fn capability_parse_rejects_unknown_family() {
    let err = Capability::parse("quantum_recall").expect_err("unknown family must not parse");
    assert!(err.contains("quantum_recall"), "unhelpful error: {err}");
}

#[test]
fn mandatory_families_are_core_recall_and_portability() {
    assert_eq!(
        Capability::MANDATORY,
        [
            Capability::Core,
            Capability::Recall,
            Capability::Portability
        ]
    );
    for capability in Capability::ALL {
        assert_eq!(
            capability.is_mandatory(),
            matches!(
                capability,
                Capability::Core | Capability::Recall | Capability::Portability
            ),
            "wrong mandatory classification for {capability}"
        );
    }
}

#[test]
fn capabilities_all_contains_every_family() {
    let all = Capabilities::all();
    assert_eq!(all.len(), Capability::ALL.len());
    for capability in Capability::ALL {
        assert!(all.contains(capability), "all() is missing {capability}");
    }
    assert!(!all.is_empty());
}

#[test]
fn capabilities_empty_contains_nothing() {
    let none = Capabilities::empty();
    assert!(none.is_empty());
    assert_eq!(none.len(), 0);
    for capability in Capability::ALL {
        assert!(!none.contains(capability));
    }
    // The `null` driver's set is the default.
    assert_eq!(Capabilities::default(), none);
}

#[test]
fn capabilities_insert_and_remove_are_idempotent() {
    let mut set = Capabilities::empty();
    set.insert(Capability::Tree);
    set.insert(Capability::Tree);
    assert_eq!(set.len(), 1);
    assert!(set.contains(Capability::Tree));
    assert!(!set.contains(Capability::Graph));

    set.remove(Capability::Tree);
    set.remove(Capability::Tree);
    assert!(set.is_empty());
}

#[test]
fn capabilities_builder_forms_mirror_insert_and_remove() {
    let set = Capabilities::empty()
        .with(Capability::Core)
        .with(Capability::Recall)
        .without(Capability::Recall);
    assert!(set.contains(Capability::Core));
    assert!(!set.contains(Capability::Recall));
}

#[test]
fn capabilities_contains_all_checks_subsets() {
    let full = Capabilities::all();
    let mandatory = Capabilities::mandatory();

    assert!(full.contains_all(mandatory));
    assert!(!mandatory.contains_all(full));
    assert!(mandatory.contains_all(mandatory));
    assert!(full.contains_all(Capabilities::empty()));
}

#[test]
fn capabilities_iterates_in_declaration_order() {
    let set: Capabilities = [
        Capability::Portability,
        Capability::Core,
        Capability::Tree,
        Capability::Recall,
    ]
    .into_iter()
    .collect();

    assert_eq!(
        set.iter().collect::<Vec<_>>(),
        vec![
            Capability::Core,
            Capability::Recall,
            Capability::Tree,
            Capability::Portability
        ]
    );
}

#[test]
fn capabilities_serde_round_trips_and_uses_a_string_array() {
    let set = Capabilities::mandatory().with(Capability::ToolMemory);
    let encoded = serde_json::to_value(set).unwrap();

    // Declaration order, snake_case strings — the `capabilities[]` handshake
    // field.
    assert_eq!(
        encoded,
        json!(["core", "recall", "tool_memory", "portability"])
    );

    let decoded: Capabilities = serde_json::from_value(encoded).unwrap();
    assert_eq!(decoded, set);
}

#[test]
fn capabilities_full_set_serde_round_trips() {
    let all = Capabilities::all();
    let encoded = serde_json::to_string(&all).unwrap();
    let decoded: Capabilities = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, all);
}

#[test]
fn capabilities_deserialization_collapses_duplicates_and_ignores_order() {
    let decoded: Capabilities =
        serde_json::from_value(json!(["portability", "core", "core", "recall"])).unwrap();
    assert_eq!(decoded, Capabilities::mandatory());
    assert_eq!(decoded.len(), 3);
}

#[test]
fn capabilities_deserialization_rejects_an_unknown_family() {
    // The set type itself is strict; a handshake parser that wants to *skip*
    // unknown families must decode the array element-wise via
    // `Capability::parse`, which is why that function returns a `Result`.
    let err = serde_json::from_value::<Capabilities>(json!(["core", "warp_drive"]))
        .expect_err("unknown family must not silently decode");
    assert!(err.to_string().contains("warp_drive"), "{err}");
}

#[test]
fn validate_accepts_the_minimum_bindable_set() {
    assert_eq!(Capabilities::mandatory().validate(), Ok(()));
    assert_eq!(Capabilities::all().validate(), Ok(()));
}

#[test]
fn validate_rejects_a_set_missing_core() {
    let set = Capabilities::all().without(Capability::Core);
    let err = set.validate().expect_err("missing core must be rejected");
    assert_eq!(err.missing, vec![Capability::Core]);
    assert!(err.to_string().contains("core"), "{err}");
}

#[test]
fn validate_rejects_a_set_missing_recall() {
    let set = Capabilities::all().without(Capability::Recall);
    let err = set.validate().expect_err("missing recall must be rejected");
    assert_eq!(err.missing, vec![Capability::Recall]);
    assert!(err.to_string().contains("recall"), "{err}");
}

#[test]
fn validate_rejects_a_set_missing_portability() {
    // Portability is mandatory because without it a bind is a one-way door.
    let set = Capabilities::all().without(Capability::Portability);
    let err = set
        .validate()
        .expect_err("missing portability must be rejected");
    assert_eq!(err.missing, vec![Capability::Portability]);
    assert!(err.to_string().contains("portability"), "{err}");
}

#[test]
fn validate_reports_every_missing_mandatory_family_at_once() {
    let err = Capabilities::empty()
        .validate()
        .expect_err("the null set must be rejected");
    assert_eq!(
        err.missing,
        vec![
            Capability::Core,
            Capability::Recall,
            Capability::Portability
        ]
    );
}

#[test]
fn missing_mandatory_converts_to_an_invalid_memory_error() {
    let err = Capabilities::empty().validate().unwrap_err();
    let message = err.to_string();
    let converted: MemoryError = err.into();
    // An incomplete advertised set is a bad claim about the driver, not an
    // unsupported call.
    assert!(matches!(converted, MemoryError::Invalid(ref m) if *m == message));
}

#[test]
fn missing_mandatory_is_empty_for_a_valid_set() {
    assert!(Capabilities::mandatory().missing_mandatory().is_empty());
    assert!(Capabilities::all().missing_mandatory().is_empty());
}
