use super::*;
use crate::memory::chunks::SourceKind;
use chrono::Utc;

fn meta(tags: &[&str]) -> Metadata {
    let mut m = Metadata::point_in_time(SourceKind::Chat, "x", "owner", Utc::now());
    m.tags = tags.iter().map(|s| s.to_string()).collect();
    m
}

#[test]
fn no_tags_neutral() {
    assert_eq!(score(&meta(&[])), 0.5);
    assert_eq!(score(&meta(&["unrelated"])), 0.5);
}

#[test]
fn sent_tag_high_score() {
    assert!((score(&meta(&["sent"])) - 0.6).abs() < 1e-6);
}

#[test]
fn stacking_capped_at_one() {
    // sent (0.6) + reply (0.5) + mention (0.2) = 1.3 → clamp to 1.0
    assert!((score(&meta(&["sent", "reply", "mention"])) - 1.0).abs() < 1e-6);
}

#[test]
fn reply_only() {
    assert!((score(&meta(&["reply"])) - 0.5).abs() < 1e-6);
}

#[test]
fn dm_plus_mention() {
    assert!((score(&meta(&["dm", "mention"])) - 0.5).abs() < 1e-6);
}
