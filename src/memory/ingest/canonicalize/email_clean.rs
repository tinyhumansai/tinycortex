//! Shared email rendering + cleaning helpers.
//!
//! Used by [`super::email`] when rendering canonical email markdown. The module
//! is intentionally pure-string-oriented plus a single `serde_json::Value`
//! helper (`parse_message_date`) for callers that work directly off slim
//! envelope JSON. Nothing here depends on the chunk-store types, which keeps the
//! helpers reusable.

use chrono::{DateTime, NaiveDate, Utc};
use serde_json::Value;

/// Two-stage cleanup applied to each message body before it gets rendered into
/// a digest:
///
/// 1. **Drop quoted reply chains** — once a message contains a
///    `On <date>, <name> wrote:` preamble, an `Original Message` /
///    `Forwarded message` separator, or a run of three+ consecutive
///    `>`-prefixed lines, everything from that point onward is the parent
///    message we already render directly above.
/// 2. **Drop footer noise** — `Unsubscribe`, `View in browser`, copyright
///    lines, legal disclaimers, and address blocks. We cut at the first line
///    containing any of [`FOOTER_TRIGGERS`].
///
/// The two passes run in order so a quoted-chain preamble below a
/// "view in browser" line still gets stripped on its own merits even if the
/// footer pass missed it.
pub fn clean_body(raw: &str) -> String {
    let stage1 = drop_reply_chain(raw);
    let stage2 = drop_footer_noise(&stage1);
    collapse_blank_runs(stage2.trim())
}

/// Substrings that, when matched (case-insensitive) anywhere on a line, mark
/// the start of footer / boilerplate territory. Conservative list — every entry
/// should be unambiguous noise that wouldn't reasonably appear inside real
/// prose.
const FOOTER_TRIGGERS: &[&str] = &[
    "unsubscribe",
    "view in browser",
    "view this email in your browser",
    "view it in your browser",
    "update your email settings",
    "manage your subscription",
    "manage preferences",
    "email preferences",
    "you are receiving this email because",
    "you received this email because",
    "you're receiving this email because",
    "to stop receiving",
    "all rights reserved",
    "© 20",
    "(c) 20",
    "copyright 20",
    "powered by mailchimp",
    "sent via sendgrid",
    "this email and any files",
    "confidentiality notice",
    "if you are not the intended recipient",
    "this communication may contain",
];

/// Strip quoted reply chains. See [`clean_body`] for details.
pub fn drop_reply_chain(s: &str) -> String {
    let mut offset = 0usize;
    let mut quoted_run_start: Option<usize> = None;
    let mut quoted_run_len = 0u32;

    for line in s.split_inclusive('\n') {
        let trimmed = line.trim();
        let lower = trimmed.to_ascii_lowercase();

        // Explicit reply / forward markers.
        let is_preamble = (lower.starts_with("on ") && lower.contains(" wrote:"))
            || lower.contains("---------- forwarded message")
            || lower.contains("----- original message")
            || lower.contains("--------- original message")
            || lower.contains("--- forwarded by");
        if is_preamble {
            debug_assert!(s.is_char_boundary(offset));
            return s[..offset].trim_end().to_string();
        }

        // Three+ consecutive lines starting with `>` is a quoted reply chain in
        // disguise (some clients de-quote on send). Treat the start of the run
        // as the cut point.
        if trimmed.starts_with('>') {
            if quoted_run_start.is_none() {
                quoted_run_start = Some(offset);
                quoted_run_len = 1;
            } else {
                quoted_run_len += 1;
            }
            if quoted_run_len >= 3 {
                let cut = quoted_run_start.unwrap_or(offset);
                debug_assert!(s.is_char_boundary(cut));
                return s[..cut].trim_end().to_string();
            }
        } else if !trimmed.is_empty() {
            // Reset on a non-empty, non-quoted line. Blank lines don't break a
            // quote run because senders often interleave them.
            quoted_run_start = None;
            quoted_run_len = 0;
        }

        offset += line.len();
    }
    s.to_string()
}

/// Strip everything from the first line containing a footer trigger onward.
/// See [`FOOTER_TRIGGERS`] for the matched list.
pub fn drop_footer_noise(s: &str) -> String {
    let mut offset = 0usize;
    for line in s.split_inclusive('\n') {
        let lower = line.to_ascii_lowercase();
        if FOOTER_TRIGGERS.iter().any(|t| lower.contains(t)) {
            debug_assert!(s.is_char_boundary(offset));
            return s[..offset].trim_end().to_string();
        }
        offset += line.len();
    }
    s.to_string()
}

/// Collapse runs of 2+ blank lines into a single blank line. Trims trailing
/// newlines.
pub fn collapse_blank_runs(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut blank = 0u32;
    for line in s.lines() {
        if line.trim().is_empty() {
            blank += 1;
            if blank <= 1 {
                out.push('\n');
            }
        } else {
            blank = 0;
            out.push_str(line);
            out.push('\n');
        }
    }
    while out.ends_with('\n') {
        out.pop();
    }
    out
}

/// Truncate a body to at most `max_chars` characters, appending `…` when the
/// body is longer. Trims first so leading/trailing whitespace doesn't count
/// against the budget.
pub fn truncate_body(body: &str, max_chars: usize) -> String {
    let trimmed = body.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let mut out: String = trimmed.chars().take(max_chars).collect();
    out.push('…');
    out
}

/// Escape only the few markdown chars that would visibly break the
/// header/inline contexts we use (#, |, *, _, `). Newlines collapse to spaces.
/// We leave most punctuation alone — the body is rendered as a blockquote
/// anyway.
pub fn md_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' | '`' | '*' | '_' | '|' => {
                out.push('\\');
                out.push(ch);
            }
            '\n' | '\r' => out.push(' '),
            _ => out.push(ch),
        }
    }
    out
}

/// Pull the `<addr@host>` portion out of a `From` header, returning just the
/// bare email address. Falls back to `None` when no `<…>` brackets exist; in
/// that case the caller may use the raw From field.
pub fn extract_email(from: &str) -> Option<String> {
    let s = from.trim();
    if let (Some(start), Some(end)) = (s.rfind('<'), s.rfind('>')) {
        if start < end {
            debug_assert!(s.is_char_boundary(start + 1));
            debug_assert!(s.is_char_boundary(end));
            let inner = s[start + 1..end].trim();
            if inner.contains('@') {
                return Some(inner.to_string());
            }
        }
    }
    if s.contains('@') && !s.contains(' ') {
        return Some(s.to_string());
    }
    None
}

/// If `s` starts with a 3-letter day-of-week prefix (`Mon, `, `Tue, `, …),
/// return the remainder; otherwise `None`. Used to feed a strict-rfc2822 reject
/// into a lenient retry.
fn strip_day_of_week_prefix(s: &str) -> Option<&str> {
    const DAYS: &[&str] = &["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    let (prefix, rest) = s.split_once(", ")?;
    if DAYS.iter().any(|d| d.eq_ignore_ascii_case(prefix)) {
        Some(rest)
    } else {
        None
    }
}

/// Try a sequence of common date formats. The slim envelope sets `date` from
/// `messageTimestamp` (often ISO 8601 or epoch ms) when present, falling back
/// to the raw `Date:` header (RFC 2822). Operates on the raw `serde_json::Value`
/// so callers that work off the slim envelope JSON don't have to reshape it
/// first.
pub fn parse_message_date(m: &Value) -> Option<DateTime<Utc>> {
    if let Some(dt) = m.get("date").and_then(parse_date_value) {
        return Some(dt);
    }
    if let Some(dt) = m.get("internalDate").and_then(parse_date_value) {
        return Some(dt);
    }
    m.get("data")
        .and_then(|data| data.get("internalDate"))
        .and_then(parse_date_value)
}

fn parse_date_value(raw: &Value) -> Option<DateTime<Utc>> {
    if let Some(s) = raw.as_str() {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }
        // Epoch millis as a string? Gmail's `internalDate` uses this form.
        if let Ok(ms) = s.parse::<i64>() {
            return DateTime::from_timestamp_millis(ms);
        }
        if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
            return Some(dt.with_timezone(&Utc));
        }
        if let Ok(dt) = DateTime::parse_from_rfc2822(s) {
            return Some(dt.with_timezone(&Utc));
        }
        // Lenient RFC 2822 fallback: strict `parse_from_rfc2822` rejects
        // mismatched day-of-week. Strip a `<DayName>, ` prefix and retry with
        // the rfc2822 body format.
        if let Some(rest) = strip_day_of_week_prefix(s) {
            if let Ok(dt) = DateTime::parse_from_str(rest, "%d %b %Y %H:%M:%S %z") {
                return Some(dt.with_timezone(&Utc));
            }
        }
        if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
            return d.and_hms_opt(0, 0, 0).map(|n| n.and_utc());
        }
    }
    raw.as_i64().and_then(DateTime::from_timestamp_millis)
}

#[cfg(test)]
#[path = "email_clean_tests.rs"]
mod tests;
