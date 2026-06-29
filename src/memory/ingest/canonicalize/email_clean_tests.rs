use super::*;
use serde_json::json;

#[test]
fn drop_reply_chain_strips_on_x_wrote_preamble() {
    let body = "Sounds good — let's do Tuesday.\n\nOn Mon, Apr 22, 2026 at 10:00 AM, Alice <a@x> wrote:\n> Tuesday or Wednesday?\n> Let me know.";
    let cleaned = drop_reply_chain(body);
    assert_eq!(cleaned.trim(), "Sounds good — let's do Tuesday.");
}

#[test]
fn drop_reply_chain_strips_forwarded_separator() {
    let body = "FYI.\n\n---------- Forwarded message ---------\nFrom: bob\nSubject: hi";
    assert_eq!(drop_reply_chain(body).trim(), "FYI.");
}

#[test]
fn drop_reply_chain_strips_consecutive_quoted_run() {
    let body = "Thanks for the update.\n\n> earlier line 1\n> earlier line 2\n> earlier line 3\n> earlier line 4";
    assert_eq!(drop_reply_chain(body).trim(), "Thanks for the update.");
}

#[test]
fn drop_reply_chain_keeps_short_quote() {
    let body = "I think:\n> That sounds reasonable\n\nLet's proceed.";
    let cleaned = drop_reply_chain(body);
    assert!(cleaned.contains("Let's proceed"));
    assert!(cleaned.contains("That sounds reasonable"));
}

#[test]
fn drop_footer_noise_strips_unsubscribe_block() {
    let body =
        "Big news: GPT-5.5 is here.\n\nRead more at example.com\n\nUnsubscribe | © 2026 OpenAI";
    let cleaned = drop_footer_noise(body);
    assert!(cleaned.contains("GPT-5.5"));
    assert!(!cleaned.to_ascii_lowercase().contains("unsubscribe"));
    assert!(!cleaned.contains("©"));
}

#[test]
fn drop_footer_noise_strips_legal_disclaimer() {
    let body = "Action item — review by Friday.\n\nThis email and any files transmitted with it are confidential and intended solely for the use of the individual to whom they are addressed.";
    let cleaned = drop_footer_noise(body);
    assert_eq!(cleaned.trim(), "Action item — review by Friday.");
}

#[test]
fn clean_body_combines_passes() {
    let body =
        "Real content here.\n\nOn Mon, Apr 22, 2026, Alice wrote:\n> old stuff\n\nUnsubscribe";
    let cleaned = clean_body(body);
    assert_eq!(cleaned, "Real content here.");
}

#[test]
fn collapse_blank_runs_keeps_paragraph_breaks() {
    let s = "a\n\n\n\nb\n\n\nc\n";
    assert_eq!(collapse_blank_runs(s), "a\n\nb\n\nc");
}

#[test]
fn truncate_body_adds_ellipsis() {
    let s = "x".repeat(2000);
    let t = truncate_body(&s, 1200);
    assert!(t.ends_with('…'));
    assert_eq!(t.chars().count(), 1201);
}

#[test]
fn truncate_body_passthrough_when_short() {
    let s = "hello";
    let t = truncate_body(s, 1200);
    assert_eq!(t, "hello");
}

#[test]
fn md_escape_handles_special_chars() {
    assert_eq!(md_escape("a*b_c"), "a\\*b\\_c");
    assert_eq!(md_escape("foo|bar"), "foo\\|bar");
    assert_eq!(md_escape("line1\nline2"), "line1 line2");
    assert_eq!(md_escape("plain text"), "plain text");
}

#[test]
fn extract_email_handles_both_forms() {
    assert_eq!(
        extract_email("Alice <alice@example.com>").as_deref(),
        Some("alice@example.com")
    );
    assert_eq!(
        extract_email("notify@github.com").as_deref(),
        Some("notify@github.com")
    );
    assert_eq!(
        extract_email("\"Bot Name\" <bot@x.io>").as_deref(),
        Some("bot@x.io")
    );
    assert!(extract_email("Alice").is_none());
}

#[test]
fn parse_message_date_handles_iso_and_rfc2822() {
    let iso = json!({"date": "2026-04-21T10:00:00Z"});
    let rfc = json!({"date": "Mon, 21 Apr 2026 10:00:00 +0000"});
    let ms = json!({"date": 1745236800000_i64});
    let ms_str = json!({"date": "1745236800000"});
    let internal_ms_str = json!({"internalDate": "1745236800000"});
    let nested_internal_ms_str = json!({"data": {"internalDate": "1745236800000"}});
    let date_only = json!({"date": "2026-04-21"});
    assert!(parse_message_date(&iso).is_some());
    assert!(parse_message_date(&rfc).is_some());
    assert!(parse_message_date(&ms).is_some());
    assert!(parse_message_date(&ms_str).is_some());
    assert!(parse_message_date(&internal_ms_str).is_some());
    assert!(parse_message_date(&nested_internal_ms_str).is_some());
    assert!(parse_message_date(&date_only).is_some());
}

#[test]
fn parse_message_date_returns_none_when_missing_or_blank() {
    assert!(parse_message_date(&json!({})).is_none());
    assert!(parse_message_date(&json!({"date": ""})).is_none());
    assert!(parse_message_date(&json!({"date": "   "})).is_none());
}

#[test]
fn drop_reply_chain_handles_zwnj_in_body() {
    let zwnj = "\u{200c}";
    let body = format!(
        "سلام{}دوست عزیز، لطفاً بررسی کنید.\n\nOn Mon, Apr 22, 2026, Alice wrote:\n> old content",
        zwnj
    );

    let cleaned = drop_reply_chain(&body);

    assert!(!cleaned.contains("old content"));
    assert!(
        cleaned.contains(zwnj),
        "ZWNJ was incorrectly removed from real content"
    );
    assert!(std::str::from_utf8(cleaned.as_bytes()).is_ok());
}
