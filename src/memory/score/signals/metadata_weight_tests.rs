use super::*;
use chrono::Utc;

fn meta(kind: SourceKind) -> Metadata {
    Metadata::point_in_time(kind, "x", "owner", Utc::now())
}

#[test]
fn per_kind_weights() {
    assert!(score(&meta(SourceKind::Document)) > score(&meta(SourceKind::Email)));
    assert!(score(&meta(SourceKind::Email)) > score(&meta(SourceKind::Chat)));
}

#[test]
fn bounded_zero_one() {
    for k in [SourceKind::Chat, SourceKind::Email, SourceKind::Document] {
        let s = score(&meta(k));
        assert!((0.0..=1.0).contains(&s));
    }
}
