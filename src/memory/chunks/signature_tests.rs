//! Tests for signature parsing, equivalence, and variant expansion.

use super::*;

#[test]
fn parses_the_canonical_spelling() {
    let parts = parse_signature("provider=cloud;model=embedding-v1;dims=1024").unwrap();
    assert_eq!(parts.provider.as_deref(), Some("cloud"));
    assert_eq!(parts.model, "embedding-v1");
    assert_eq!(parts.dims, Some(1024));
}

#[test]
fn parses_the_legacy_spelling() {
    let parts = parse_signature("embedding-v1@1024").unwrap();
    assert_eq!(parts.provider, None);
    assert_eq!(parts.model, "embedding-v1");
    assert_eq!(parts.dims, Some(1024));
}

#[test]
fn a_model_id_containing_an_at_sign_keeps_it() {
    // Split at the LAST `@`, so `openai@v2@1536` is model `openai@v2`.
    let parts = parse_signature("openai@v2@1536").unwrap();
    assert_eq!(parts.model, "openai@v2");
    assert_eq!(parts.dims, Some(1536));
}

#[test]
fn an_unrecognised_shape_degrades_to_a_bare_model_name() {
    let parts = parse_signature("some-third-convention").unwrap();
    assert_eq!(parts.model, "some-third-convention");
    assert_eq!(parts.dims, None);
    // …and still compares equal to itself, so such a store keeps its vectors.
    assert!(signatures_equivalent(
        "some-third-convention",
        "some-third-convention"
    ));
    assert!(parse_signature("   ").is_none());
}

#[test]
fn the_two_spellings_of_one_space_are_equivalent() {
    assert!(signatures_equivalent(
        "provider=cloud;model=embedding-v1;dims=1024",
        "embedding-v1@1024"
    ));
    // …in both directions, and regardless of which side carries the provider.
    assert!(signatures_equivalent(
        "embedding-v1@1024",
        "provider=cloud;model=embedding-v1;dims=1024"
    ));
}

#[test]
fn a_different_model_or_dimension_is_a_different_space() {
    assert!(!signatures_equivalent(
        "provider=cloud;model=embedding-v1;dims=1024",
        "nomic-embed-text@1024"
    ));
    assert!(!signatures_equivalent(
        "provider=cloud;model=embedding-v1;dims=1024",
        "embedding-v1@768"
    ));
    // Two *declared* providers that disagree stay separate.
    assert!(!signatures_equivalent(
        "provider=cloud;model=embedding-v1;dims=1024",
        "provider=ollama;model=embedding-v1;dims=1024"
    ));
}

#[test]
fn variants_cover_both_spellings_with_the_input_first() {
    let variants = signature_variants("provider=cloud;model=embedding-v1;dims=1024");
    assert_eq!(
        variants,
        vec![
            "provider=cloud;model=embedding-v1;dims=1024".to_string(),
            "embedding-v1@1024".to_string(),
        ]
    );

    // From the legacy side the canonical spelling is unknowable (no provider),
    // so the expansion is just the input — matching stays correct, it simply
    // cannot invent a provider it was never told.
    assert_eq!(
        signature_variants("embedding-v1@1024"),
        vec!["embedding-v1@1024".to_string()]
    );
}

#[test]
fn variants_never_repeat_a_spelling() {
    // `dim` (singular) parses, so the canonical rebuild would duplicate the
    // legacy form if dedup were missing.
    let variants = signature_variants("embedding-v1@1024");
    let mut sorted = variants.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), variants.len(), "variants must be unique");
}

#[test]
fn the_canonical_spelling_matches_the_namespace_store_byte_for_byte() {
    // The whole point of the unification: one space, one string, both stores.
    assert_eq!(
        format_signature("cloud", "embedding-v1", 1024),
        crate::memory::store::vectors::format_embedding_signature("cloud", "embedding-v1", 1024)
    );
}

#[test]
fn in_clause_numbers_placeholders_from_the_given_index() {
    assert_eq!(signature_in_clause(2, 1), "IN (?1, ?2)");
    assert_eq!(signature_in_clause(2, 5), "IN (?5, ?6)");
    assert_eq!(signature_in_clause(1, 3), "IN (?3)");
}
