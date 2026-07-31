//! Tests for the page-size retry helpers.

use serde_json::json;

use super::*;

#[test]
fn payload_too_large_is_told_apart_from_other_provider_errors() {
    assert!(is_payload_too_large(Some(
        "413 {\"slug\":\"Upstream_PayloadTooLarge\"}"
    )));
    assert!(is_payload_too_large(Some("Response too large for tool")));
    assert!(!is_payload_too_large(Some("rate limit exceeded")));
    assert!(!is_payload_too_large(Some("invalid grant")));
    assert!(!is_payload_too_large(None));
}

#[test]
fn shrinking_stops_at_the_floor() {
    let mut arguments = json!({ "max_results": 3 });
    assert_eq!(
        shrink_page_size(&mut arguments, Some("max_results")),
        Some(1)
    );
    assert_eq!(arguments["max_results"], json!(1));
    // At one item per page there is nothing left to halve.
    assert_eq!(shrink_page_size(&mut arguments, Some("max_results")), None);
    // A source that declares no key, or a request that lacks it, cannot shrink.
    assert_eq!(shrink_page_size(&mut arguments, None), None);
    assert_eq!(
        shrink_page_size(&mut json!({ "other": 10 }), Some("max_results")),
        None
    );
}

/// The digits have to stand alone. Every false positive here costs a
/// shrink-and-retry cycle on a failure a smaller page was never going to fix,
/// and delays the real error reaching the caller.
#[test]
fn a_number_that_merely_contains_413_is_not_a_status_code() {
    assert!(is_payload_too_large(Some("HTTP 413 Payload Too Large")));
    assert!(is_payload_too_large(Some("upstream returned 413.")));
    assert!(is_payload_too_large(Some("(413)")));

    assert!(!is_payload_too_large(Some("message id 4130 not found")));
    assert!(!is_payload_too_large(Some("amount 1413 exceeds the cap")));
    assert!(!is_payload_too_large(Some("thread 94137 is archived")));
    assert!(!is_payload_too_large(Some(
        "at 1782891413 the token expired"
    )));
}
