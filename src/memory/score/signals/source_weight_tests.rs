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
