use super::*;
use chrono::Utc;

fn meta_with_tag(kind: SourceKind, tag: &str) -> Metadata {
    let mut m = Metadata::point_in_time(kind, "x", "owner", Utc::now());
    m.tags.push(tag.to_string());
    m
}

#[test]
fn data_source_inferred_from_tags() {
    let m = meta_with_tag(SourceKind::Chat, "provider:whatsapp");
    assert_eq!(infer_data_source(&m), Some(DataSource::Whatsapp));
}

#[test]
fn plain_user_label_does_not_infer_provider() {
    let m = meta_with_tag(SourceKind::Email, "notion");
    assert_eq!(infer_data_source(&m), None);
    assert!((score(&m) - 0.75).abs() < 1e-6);
}

#[test]
fn unknown_tag_falls_back_to_kind_default() {
    let m = meta_with_tag(SourceKind::Email, "not-a-data-source");
    let s = score(&m);
    assert!((s - 0.75).abs() < 1e-6);
}

#[test]
fn provider_specific_weights_applied() {
    let m = meta_with_tag(SourceKind::Document, "provider:meeting_notes");
    assert!((score(&m) - 0.85).abs() < 1e-6);
}

#[test]
fn all_data_sources_bounded() {
    for ds in DataSource::all() {
        let w = weight_for(*ds);
        assert!((0.0..=1.0).contains(&w));
    }
}

/// Recovers the compile-time exhaustiveness guarantee that `DataSource` lost
/// when it moved into the `tinycortex-api` contract crate.
///
/// `#[non_exhaustive]` is inert inside its defining crate but binding across a
/// crate boundary, so `weight_for_explicit`'s match now needs a wildcard and
/// the compiler can no longer prove every provider has a hand-tuned weight.
/// Without this test, adding a provider to the contract crate would silently
/// degrade it to the coarse `kind_default` instead of failing the build.
#[test]
fn every_data_source_has_an_explicit_weight() {
    let missing: Vec<&'static str> = DataSource::all()
        .iter()
        .filter(|ds| weight_for_explicit(**ds).is_none())
        .map(|ds| ds.as_str())
        .collect();
    assert!(
        missing.is_empty(),
        "these DataSource variants fall through to kind_default instead of a \
         hand-tuned weight; add them to weight_for_explicit: {missing:?}"
    );
}
