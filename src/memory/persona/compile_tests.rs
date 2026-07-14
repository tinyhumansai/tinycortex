//! Tests for the deterministic persona compiler.

use super::*;

fn inputs_with(bodies: &[(PersonaFacet, &str)]) -> PackInputs {
    let mut inputs = PackInputs::new("test@example.com");
    for (f, b) in bodies {
        inputs.facet_bodies.insert(*f, b.to_string());
        inputs.counts.insert(*f, 5);
        inputs.scopes.insert(*f, 2);
    }
    inputs
}

#[test]
fn emits_header_and_sections_in_fixed_order() {
    let inputs = inputs_with(&[
        (PersonaFacet::Stack, "- Uses Rust 2021."),
        (
            PersonaFacet::Directives,
            "- Always branch before writing code.",
        ),
        (PersonaFacet::CodingStyle, "- Small focused modules."),
    ]);
    let pack = compile_pack(&inputs);

    assert!(pack.starts_with("# Persona: test@example.com"));
    // Directives must come before Coding style, which comes before Stack.
    let d = pack.find("## Directives").unwrap();
    let c = pack.find("## Coding style").unwrap();
    let s = pack.find("## Stack").unwrap();
    assert!(d < c && c < s, "section order wrong:\n{pack}");
    // Strength annotation is present.
    assert!(pack.contains("distilled from 5 observations across 2 projects"));
    // Verbatim directive survives.
    assert!(pack.contains("Always branch before writing code."));
}

#[test]
fn empty_inputs_produce_placeholder() {
    let pack = compile_pack(&PackInputs::new("nobody"));
    assert!(pack.contains("# Persona: nobody"));
    assert!(pack.contains("No persona evidence distilled yet"));
    assert!(!pack.contains("## "));
}

#[test]
fn total_budget_ceiling_protects_directives_first() {
    // A huge body per facet; a tiny total budget. Directives (first) must
    // survive; later facets get dropped.
    let big = "- ".to_string() + &"word ".repeat(4000);
    let mut inputs = inputs_with(&[
        (PersonaFacet::Directives, &big),
        (PersonaFacet::Stack, &big),
        (PersonaFacet::AntiPreferences, &big),
    ]);
    inputs.per_facet_budget = 2_000;
    inputs.total_budget_max = 2_100; // only room for ~one facet
    let pack = compile_pack(&inputs);
    assert!(
        pack.contains("## Directives"),
        "directives must be protected"
    );
    assert!(
        !pack.contains("## Anti-preferences"),
        "later facets dropped under the ceiling:\n{}",
        &pack[..pack.len().min(200)]
    );
}

#[test]
fn per_facet_body_is_clamped() {
    let big = "- ".to_string() + &"token ".repeat(3000);
    let mut inputs = inputs_with(&[(PersonaFacet::Directives, &big)]);
    inputs.per_facet_budget = 100;
    inputs.total_budget_max = 10_000;
    let pack = compile_pack(&inputs);
    // The clamped directives section must be far smaller than the raw body.
    assert!(
        pack.len() < big.len() / 2,
        "body was not clamped: {} vs {}",
        pack.len(),
        big.len()
    );
}

#[test]
fn deterministic_across_runs() {
    let inputs = inputs_with(&[
        (PersonaFacet::Communication, "- Terse and direct."),
        (
            PersonaFacet::Workflow,
            "- Uses worktrees for parallel work.",
        ),
    ]);
    assert_eq!(compile_pack(&inputs), compile_pack(&inputs));
}
