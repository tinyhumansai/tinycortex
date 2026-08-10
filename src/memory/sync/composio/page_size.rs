//! Page-size retry for a provider that refuses a page for its size.
//!
//! A page rejected purely because the response is too big is the one provider
//! failure a *smaller request* can fix. The orchestrator halves the page-size
//! argument and retries rather than failing the source, so a single oversized
//! page cannot stop a sync dead — on one live workspace a Gmail sync sat broken
//! for nine days that way.

use serde_json::Value;

/// Smallest page a shrink will ask for. One item is the point past which a
/// too-large response is about that single item, not the batch size.
pub(super) const MIN_PAGE_SIZE: u64 = 1;

/// Whether the provider refused a page for its *size* rather than for anything
/// about the request's content — the only failure a smaller page can fix.
///
/// Matched on the error text because that is all the envelope carries: Composio
/// reports it as HTTP 413 with a `Upstream_PayloadTooLarge` slug, and other
/// backends phrase it as "payload too large" / "response too large".
pub(super) fn is_payload_too_large(error: Option<&str>) -> bool {
    error.is_some_and(|error| {
        let lower = error.to_ascii_lowercase();
        lower.contains("payloadtoolarge")
            || lower.contains("payload_too_large")
            || mentions_status_413(&lower)
            || (lower.contains("too large")
                && (lower.contains("payload") || lower.contains("response")))
    })
}

/// Whether `text` names HTTP 413 as a status code.
///
/// The digits have to stand alone. An unanchored `contains("413")` also matches
/// a message id, an amount, or a timestamp that merely contains those three
/// digits, and every such match costs a shrink-and-retry cycle before the real
/// error is finally surfaced — on a failure that a smaller page was never going
/// to fix.
fn mentions_status_413(lower: &str) -> bool {
    lower.match_indices("413").any(|(at, _)| {
        let before_is_digit = lower[..at]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_digit());
        let after_is_digit = lower[at + 3..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit());
        !before_is_digit && !after_is_digit
    })
}

/// Pin the page-size argument to `size`, if the source declared one.
pub(super) fn apply_page_size(arguments: &mut Value, key: Option<&str>, size: Option<u64>) {
    if let (Some(key), Some(size)) = (key, size) {
        if let Some(slot) = arguments.get_mut(key) {
            *slot = Value::from(size);
        }
    }
}

/// Halve the page-size argument in place, returning the new value.
///
/// `None` means retrying is pointless — the source declares no page-size
/// argument, this request does not carry it, or it is already at the floor.
pub(super) fn shrink_page_size(arguments: &mut Value, key: Option<&str>) -> Option<u64> {
    let key = key?;
    let current = arguments.get(key)?.as_u64()?;
    if current <= MIN_PAGE_SIZE {
        return None;
    }
    let reduced = (current / 2).max(MIN_PAGE_SIZE);
    arguments[key] = Value::from(reduced);
    Some(reduced)
}

#[cfg(test)]
#[path = "page_size_tests.rs"]
mod tests;
