//! Log-side scrubber for free-form `reason` / `last_error` strings the worker
//! emits on the defer / fail branches.
//!
//! Persisted DB state (`mem_tree_jobs.last_error`) keeps the full original
//! string for diagnostics — handlers may attach upstream-provider responses or
//! full anyhow chains there. Anything destined for a lower-trust sink (a log
//! line, a bug report) is passed through [`scrub_for_log`] first to:
//!
//! 1. Mask credential-shaped tokens (Bearer, OpenAI `sk-…`, GitHub `ghp_…`,
//!    Slack `xox?-…`, generic `api_key=…` / `password=…` / `token=…`).
//! 2. Strip URL userinfo (`https://user:pass@host` → `https://***@host`).
//! 3. Mask bare email addresses.
//! 4. Cap the string at [`MAX_LEN`] bytes, suffixed with
//!    `…(truncated, N more bytes)` so a reader knows it was longer.
//!
//! Ported verbatim from OpenHuman's `memory_queue::redact`.

use std::sync::OnceLock;

use regex::Regex;

/// Upper bound on the byte length of a scrubbed string.
pub(crate) const MAX_LEN: usize = 1024;

/// Scrub a free-form error / reason string for emission to a lower-trust sink.
pub fn scrub_for_log(input: &str) -> String {
    let mut out = input.to_owned();
    for (re, replacement) in patterns() {
        out = re.replace_all(&out, *replacement).into_owned();
    }
    truncate(out)
}

/// Cap `s` at [`MAX_LEN`] bytes, appending a `…(truncated, N more bytes)`
/// suffix when it was cut. No-op when `s` already fits.
fn truncate(mut s: String) -> String {
    if s.len() <= MAX_LEN {
        return s;
    }
    // Round down to a char boundary to avoid splitting a multi-byte UTF-8
    // sequence — `String::truncate` panics on a non-boundary.
    let mut cut = MAX_LEN;
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    let dropped = s.len() - cut;
    s.truncate(cut);
    s.push_str(&format!("…(truncated, {dropped} more bytes)"));
    s
}

/// Lazily-compiled, process-lifetime regex/replacement pairs applied in order
/// by [`scrub_for_log`]. Compiled once via [`OnceLock`] since `Regex::new` is
/// not cheap and this runs on every scrubbed string.
fn patterns() -> &'static [(Regex, &'static str)] {
    static PATTERNS: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        vec![
            // URL userinfo: scheme + :// then userinfo up to '@'.
            (
                Regex::new(r"(?P<scheme>[a-zA-Z][a-zA-Z0-9+.\-]*://)[^\s/@]+@").unwrap(),
                "$scheme***@",
            ),
            // Bearer tokens (with or without an `Authorization:` prefix).
            (
                Regex::new(r"(?i)bearer\s+[A-Za-z0-9._\-+/=]+").unwrap(),
                "Bearer ***",
            ),
            // Provider-prefixed credentials with stable, well-known shapes.
            (Regex::new(r"sk-[A-Za-z0-9_\-]{16,}").unwrap(), "sk-***"),
            (Regex::new(r"ghp_[A-Za-z0-9]{20,}").unwrap(), "ghp_***"),
            (Regex::new(r"ghs_[A-Za-z0-9]{20,}").unwrap(), "ghs_***"),
            (Regex::new(r"gho_[A-Za-z0-9]{20,}").unwrap(), "gho_***"),
            (
                Regex::new(r"xox[abprs]-[A-Za-z0-9\-]{8,}").unwrap(),
                "xox-***",
            ),
            // Generic `key=value` assignments where the key implies a secret.
            (
                Regex::new(
                    r#"(?i)(?P<k>api[_\-]?key|password|passwd|pwd|secret|token|auth)\s*[:=]\s*(?:"[^"]*"|'[^']*'|[^\s,}\)\]]+)"#,
                )
                .unwrap(),
                "$k=***",
            ),
            // Bare email addresses.
            (
                Regex::new(r"\b[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,}\b").unwrap(),
                "***@***",
            ),
        ]
    })
}

#[cfg(test)]
#[path = "redact_tests.rs"]
mod tests;
