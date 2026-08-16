//! Tests for the Gmail message → canonical Markdown adapter.

use serde_json::json;

use super::{canonical_markdown, message_body, message_recipients, message_sent_at};

/// One message in the shape the Gmail response reshaper emits: a slim envelope
/// whose body is pre-rendered into `markdown`.
fn slim_message() -> serde_json::Value {
    json!({
        "id": "18f0abc",
        "threadId": "18f0abc",
        "subject": "Boulder visit",
        "from": "Advising <advising@colorado.edu>",
        "to": "me@example.com, second@example.com",
        "date": "2026-05-02T09:15:00Z",
        "labels": ["INBOX"],
        "markdown": "The University of Colorado orientation is on May 20.\n\nOn Fri, 1 May 2026, someone wrote:\n> please ignore this quoted reply",
    })
}

#[test]
fn canonical_markdown_renders_headers_and_cleaned_body() {
    let content = canonical_markdown(&slim_message(), "18f0abc");

    // The literal words of the mail — the thing recall has to match — are
    // present as prose, and the headers are readable rather than a MIME tree.
    assert!(
        content.contains("The University of Colorado orientation is on May 20."),
        "body text must survive canonicalisation: {content}"
    );
    assert!(
        content.contains("From: Advising <advising@colorado.edu>"),
        "{content}"
    );
    assert!(content.contains("Subject: Boulder visit"), "{content}");
    assert!(
        content.contains("To: me@example.com, second@example.com"),
        "{content}"
    );

    // Canonicalisation is what strips the quoted reply chain.
    assert!(
        !content.contains("please ignore this quoted reply"),
        "reply chain must be stripped by clean_body: {content}"
    );

    // Nothing JSON-shaped is left: this is the regression the fix exists for.
    assert!(
        !content.contains("\"markdown\""),
        "raw JSON must not be stored: {content}"
    );
    assert!(
        !content.contains("threadId"),
        "envelope keys must not be stored: {content}"
    );
}

#[test]
fn body_falls_back_to_message_text_when_the_reshape_did_not_run() {
    // No `markdown` field — the provider's own plain text is used instead.
    let raw = json!({
        "id": "18f0def",
        "subject": "Direct",
        "messageText": "Plain provider text about Colorado.",
    });
    assert_eq!(message_body(&raw), "Plain provider text about Colorado.");
    assert!(canonical_markdown(&raw, "18f0def").contains("Plain provider text about Colorado."));
}

#[test]
fn body_skips_a_present_but_blank_field() {
    // A blank `markdown` must not shadow a usable `messageText`: the candidate
    // list falls through on emptiness, not just on absence.
    let raw = json!({ "markdown": "   ", "messageText": "real body" });
    assert_eq!(message_body(&raw), "real body");
}

#[test]
fn recipients_split_from_either_a_header_string_or_an_array() {
    let joined = json!({ "to": "a@x.com, b@y.com" });
    assert_eq!(message_recipients(&joined), vec!["a@x.com", "b@y.com"]);

    let array = json!({ "to": ["a@x.com", " b@y.com "] });
    assert_eq!(message_recipients(&array), vec!["a@x.com", "b@y.com"]);

    assert!(message_recipients(&json!({})).is_empty());
}

#[test]
fn sent_at_reads_epoch_millis_and_is_deterministic_when_undated() {
    // Gmail's `internalDate` is epoch millis as a string.
    let dated = json!({ "internalDate": "1777712100000" });
    assert_eq!(message_sent_at(&dated).timestamp(), 1_777_712_100);

    // Undated messages must resolve to the same value every sync, otherwise the
    // rendered `Date:` header changes and the document re-chunks forever.
    let undated = json!({ "subject": "no date anywhere" });
    assert_eq!(message_sent_at(&undated), message_sent_at(&undated));
    assert_eq!(message_sent_at(&undated).timestamp(), 0);
}
