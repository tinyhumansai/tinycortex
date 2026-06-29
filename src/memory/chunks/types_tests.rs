//! Unit tests for the chunk model (`super`).

use super::*;

#[test]
fn chunk_id_is_deterministic() {
    let a = chunk_id(SourceKind::Chat, "slack:#eng", 0, "hello");
    let b = chunk_id(SourceKind::Chat, "slack:#eng", 0, "hello");
    assert_eq!(a, b);
    assert_eq!(a.len(), 32);
}

#[test]
fn conservative_estimate_weights_by_char_class() {
    assert_eq!(conservative_token_estimate("abcd"), 2); // 4 alnum × 2q / 4
    assert_eq!(conservative_token_estimate("    "), 1); // 4 ws × 1q / 4
    assert_eq!(conservative_token_estimate("....,,,,"), 8); // 8 punct × 4q / 4
    assert_eq!(conservative_token_estimate("שלום"), 4); // 4 non-ascii × 4q / 4
    assert_eq!(conservative_token_estimate(""), 0);
}

#[test]
fn conservative_estimate_exceeds_approx_for_dense_content() {
    let dense = "claude-memory:openhuman:MEMORY.md:67d6fe2727d431b16d41630babfdcf1cdf61bda7b9ba\n"
        .repeat(40);
    assert!(
        conservative_token_estimate(&dense) > approx_token_count(&dense),
        "conservative estimate must exceed chars/4 on dense content",
    );
}

#[test]
fn truncate_respects_budget_and_char_boundaries() {
    let text = "שלום עולם ".repeat(100); // Hebrew, ~1 token/char
    let out = truncate_to_conservative_tokens(&text, 10);
    assert!(conservative_token_estimate(out) <= 10);
    assert!(text.starts_with(out)); // valid prefix on a char boundary
    assert!(out.len() < text.len());
}

#[test]
fn truncate_is_noop_within_budget() {
    let text = "short and sweet";
    assert_eq!(truncate_to_conservative_tokens(text, 1000), text);
}

#[test]
fn chunk_id_varies_with_seq() {
    let a = chunk_id(SourceKind::Chat, "slack:#eng", 0, "hello");
    let b = chunk_id(SourceKind::Chat, "slack:#eng", 1, "hello");
    assert_ne!(a, b);
}

#[test]
fn chunk_id_varies_with_source_kind() {
    let a = chunk_id(SourceKind::Chat, "foo", 0, "hello");
    let b = chunk_id(SourceKind::Email, "foo", 0, "hello");
    assert_ne!(a, b);
}

#[test]
fn chunk_id_varies_with_source_id() {
    let a = chunk_id(SourceKind::Chat, "x", 0, "hello");
    let b = chunk_id(SourceKind::Chat, "y", 0, "hello");
    assert_ne!(a, b);
}

#[test]
fn chunk_id_varies_with_content() {
    let a = chunk_id(SourceKind::Chat, "slack:c1", 0, "bucket A content");
    let b = chunk_id(SourceKind::Chat, "slack:c1", 0, "bucket B content");
    assert_ne!(a, b);
}

#[test]
fn source_kind_round_trip() {
    for kind in [SourceKind::Chat, SourceKind::Email, SourceKind::Document] {
        assert_eq!(SourceKind::parse(kind.as_str()).unwrap(), kind);
    }
}

#[test]
fn data_source_round_trip() {
    for ds in DataSource::all() {
        assert_eq!(DataSource::parse(ds.as_str()).unwrap(), *ds);
    }
}

#[test]
fn data_source_has_all_variants() {
    assert_eq!(DataSource::all().len(), 9);
}

#[test]
fn data_source_kind_mapping() {
    use DataSource::*;
    for ds in [Discord, Telegram, Whatsapp, Conversation] {
        assert_eq!(ds.kind(), SourceKind::Chat);
    }
    for ds in [Gmail, OtherEmail] {
        assert_eq!(ds.kind(), SourceKind::Email);
    }
    for ds in [Notion, MeetingNotes, DriveDocs] {
        assert_eq!(ds.kind(), SourceKind::Document);
    }
}

#[test]
fn data_source_parse_rejects_unknown() {
    assert!(DataSource::parse("nope").is_err());
    assert!(DataSource::parse("Discord").is_err()); // case-sensitive
    assert!(DataSource::parse("drive docs").is_err()); // no spaces
}

#[test]
fn data_source_serde_is_snake_case() {
    let ds = DataSource::MeetingNotes;
    let json = serde_json::to_string(&ds).unwrap();
    assert_eq!(json, "\"meeting_notes\"");
    let parsed: DataSource = serde_json::from_str("\"meeting_notes\"").unwrap();
    assert_eq!(parsed, ds);
}

#[test]
fn approx_token_count_scales_linearly() {
    assert_eq!(approx_token_count(""), 0);
    assert_eq!(approx_token_count("a"), 1); // 1→1
    assert_eq!(approx_token_count("abcd"), 1); // 4→1
    assert_eq!(approx_token_count("abcde"), 2); // 5→2
    assert_eq!(approx_token_count(&"x".repeat(400)), 100);
}
