//! Secret-detection and redaction helpers for memory writes.
//!
//! Ported from OpenHuman's `memory_store::safety`. Conservative by design — it
//! prefers false positives over leaking credentials into long-lived stores.
//!
//! The exhaustive multilingual national-ID PII module (`safety::pii`, ~1k lines
//! of checksum logic) is ported from OpenHuman and runs as part of
//! [`sanitize_text`]. The write-rejection boundary stays stricter than content
//! scrubbing: formatted national IDs are rejected, while phone/email-like text is
//! scrubbed from content without rejecting every write that mentions them.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

/// Exhaustive checksum-gated multilingual national-ID PII module (ported from
/// OpenHuman). Content scrubbing runs from [`sanitize_text`]; the boundary
/// check is re-exported as [`has_likely_pii`].
pub mod pii;

pub use pii::{has_likely_email, has_likely_pii};

const REDACTED_SECRET: &str = "[REDACTED_SECRET]";
const REDACTED_PRIVATE_KEY: &str = "[REDACTED_PRIVATE_KEY]";
const MAX_JSON_SANITIZE_DEPTH: usize = 128;

/// Tally of what a sanitization pass changed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SanitizationReport {
    /// Count of secret/token pattern matches rewritten in string text by the
    /// [`REDACTION_PATTERNS`] pass.
    pub text_redactions: usize,
    /// Count of JSON object entries dropped wholesale because their key was
    /// classified as sensitive by [`is_sensitive_key`].
    pub key_redactions: usize,
    /// Count of full private-key blocks ([`BLOCK_PATTERNS`]) replaced; these are
    /// the most severe hits since the entire block is removed.
    pub blocked_secret_hits: usize,
    /// Count of nodes collapsed because JSON nesting reached
    /// [`MAX_JSON_SANITIZE_DEPTH`]; the subtree is replaced rather than walked.
    pub depth_redactions: usize,
    /// Count of personal-identifier matches ([`PII_PATTERNS`]) replaced by the
    /// lightweight PII screen.
    pub pii_redactions: usize,
}

impl SanitizationReport {
    /// True when any field recorded a redaction.
    pub fn changed(&self) -> bool {
        self.text_redactions > 0
            || self.key_redactions > 0
            || self.blocked_secret_hits > 0
            || self.depth_redactions > 0
            || self.pii_redactions > 0
    }

    /// Sum two reports field-wise.
    pub fn merge(self, rhs: Self) -> Self {
        Self {
            text_redactions: self.text_redactions + rhs.text_redactions,
            key_redactions: self.key_redactions + rhs.key_redactions,
            blocked_secret_hits: self.blocked_secret_hits + rhs.blocked_secret_hits,
            depth_redactions: self.depth_redactions + rhs.depth_redactions,
            pii_redactions: self.pii_redactions + rhs.pii_redactions,
        }
    }
}

/// A sanitized value plus the [`SanitizationReport`] describing the changes.
#[derive(Debug, Clone)]
pub struct Sanitized<T> {
    /// The cleaned value with secrets and PII removed.
    pub value: T,
    /// Tally of what the sanitization pass changed to produce `value`.
    pub report: SanitizationReport,
}

static BLOCK_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        Regex::new(
            r"(?is)-----BEGIN(?: [A-Z]+)? PRIVATE KEY-----.*?-----END(?: [A-Z]+)? PRIVATE KEY-----",
        )
        .expect("valid private key block"),
        Regex::new(r"(?is)-----BEGIN OPENSSH PRIVATE KEY-----.*?-----END OPENSSH PRIVATE KEY-----")
            .expect("valid openssh private key block"),
        Regex::new(
            r"(?is)-----BEGIN PGP PRIVATE KEY BLOCK-----.*?-----END PGP PRIVATE KEY BLOCK-----",
        )
        .expect("valid pgp private key block"),
    ]
});

static REDACTION_PATTERNS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    vec![
        (
            Regex::new(r"(?i)(bearer\s+)[A-Za-z0-9._~+/=-]{8,}").expect("valid bearer redaction"),
            "${1}[REDACTED]",
        ),
        (
            Regex::new(r#"(?i)(api[_-]?key\s*[=:\s]\s*["']?)[^\s"']+"#)
                .expect("valid api key redaction"),
            "${1}[REDACTED]",
        ),
        (
            Regex::new(
                r#"(?i)\b(token|access[_-]?token|refresh[_-]?token|client[_-]?secret|password|secret)\b\s*[=:\s]\s*["']?[^\s"'&]+"#,
            )
            .expect("valid token redaction"),
            "[REDACTED]",
        ),
        (
            Regex::new(r"\bsk-[A-Za-z0-9]{20,}\b").expect("valid openai key redaction"),
            "[REDACTED]",
        ),
        (
            Regex::new(r"\bgh[pousr]_[A-Za-z0-9_]{20,}\b").expect("valid github token redaction"),
            "[REDACTED]",
        ),
        (
            Regex::new(r"\bAKIA[0-9A-Z]{16}\b").expect("valid aws key redaction"),
            "[REDACTED]",
        ),
        (
            Regex::new(r"\bASIA[0-9A-Z]{16}\b").expect("valid aws sts key redaction"),
            "[REDACTED]",
        ),
        (
            Regex::new(r"\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9._-]{8,}\.[A-Za-z0-9._-]{8,}\b")
                .expect("valid jwt redaction"),
            "[REDACTED]",
        ),
        (
            Regex::new(
                r#"(?i)\b(access_token|refresh_token|id_token|authorization_code|code_verifier|code_challenge)\b\s*[=:\s]\s*["']?[^\s"'&]+"#,
            )
            .expect("valid oauth token redaction"),
            "[REDACTED]",
        ),
        (
            Regex::new(r"\bAIza[0-9A-Za-z\-_]{35}\b").expect("valid google api key redaction"),
            "[REDACTED]",
        ),
        (
            Regex::new(r"\bsk-ant-[A-Za-z0-9\-_]{16,}\b").expect("valid anthropic key redaction"),
            "[REDACTED]",
        ),
        (
            Regex::new(r"\bsk-(?:proj|org)-[A-Za-z0-9\-_]{12,}\b")
                .expect("valid openai scoped key redaction"),
            "[REDACTED]",
        ),
        (
            Regex::new(r"\b(?:sk|rk)_(?:live|test)_[A-Za-z0-9]{16,}\b")
                .expect("valid stripe key redaction"),
            "[REDACTED]",
        ),
        (
            Regex::new(r"\bxox(?:a|b|p|s|r)-[A-Za-z0-9-]{10,}\b")
                .expect("valid slack token redaction"),
            "[REDACTED]",
        ),
        (
            Regex::new(r"\bgithub_pat_[A-Za-z0-9_]{20,}\b").expect("valid github pat redaction"),
            "[REDACTED]",
        ),
        (
            Regex::new(r"\bglpat-[A-Za-z0-9\-_]{16,}\b").expect("valid gitlab pat redaction"),
            "[REDACTED]",
        ),
        (
            Regex::new(r"\bnpm_[A-Za-z0-9]{20,}\b").expect("valid npm token redaction"),
            "[REDACTED]",
        ),
        (
            Regex::new(r"\bSG\.[A-Za-z0-9_\-]{16,}\.[A-Za-z0-9_\-]{16,}\b")
                .expect("valid sendgrid key redaction"),
            "[REDACTED]",
        ),
    ]
});

/// True when `value` looks like it contains a credential.
pub fn has_likely_secret(value: &str) -> bool {
    BLOCK_PATTERNS.iter().any(|p| p.is_match(value))
        || REDACTION_PATTERNS.iter().any(|(p, _)| p.is_match(value))
}

/// Scrub secrets and PII from free text, returning the cleaned text plus a
/// [`SanitizationReport`].
pub fn sanitize_text(value: &str) -> Sanitized<String> {
    let mut out = value.to_string();
    let mut report = SanitizationReport::default();

    for pattern in BLOCK_PATTERNS.iter() {
        let hits = pattern.find_iter(&out).count();
        if hits > 0 {
            report.blocked_secret_hits += hits;
            out = pattern.replace_all(&out, REDACTED_PRIVATE_KEY).into_owned();
        }
    }

    for (pattern, replacement) in REDACTION_PATTERNS.iter() {
        let hits = pattern.find_iter(&out).count();
        if hits > 0 {
            report.text_redactions += hits;
            out = pattern.replace_all(&out, *replacement).into_owned();
        }
    }

    // Full multilingual national-ID PII scrub (checksum-gated, normalization
    // pre-pass) — runs after secret redaction so every call site that scrubs
    // secrets also scrubs PII.
    let pii = pii::redact_pii(&out);
    report = report.merge(pii.report);
    out = pii.value;

    Sanitized { value: out, report }
}

/// Recursively scrub a JSON value: sensitive keys are replaced wholesale and
/// every string value runs through [`sanitize_text`].
pub fn sanitize_json(value: &Value) -> Sanitized<Value> {
    sanitize_json_inner(value, 0)
}

/// Recursive worker behind [`sanitize_json`].
///
/// `depth` counts nesting from the call in `sanitize_json` (which starts at
/// `0`); once it reaches [`MAX_JSON_SANITIZE_DEPTH`] the whole subtree at that
/// point is replaced by a single redaction marker rather than walked further,
/// bounding recursion against pathologically deep or adversarial JSON.
fn sanitize_json_inner(value: &Value, depth: usize) -> Sanitized<Value> {
    if depth >= MAX_JSON_SANITIZE_DEPTH {
        return Sanitized {
            value: Value::String(REDACTED_SECRET.to_string()),
            report: SanitizationReport {
                depth_redactions: 1,
                ..SanitizationReport::default()
            },
        };
    }

    match value {
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            let mut report = SanitizationReport::default();
            for (key, value) in map {
                if is_sensitive_key(key) {
                    report.key_redactions += 1;
                    out.insert(key.clone(), Value::String(REDACTED_SECRET.to_string()));
                    continue;
                }
                let sanitized = sanitize_json_inner(value, depth + 1);
                report = report.merge(sanitized.report);
                out.insert(key.clone(), sanitized.value);
            }
            Sanitized {
                value: Value::Object(out),
                report,
            }
        }
        Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            let mut report = SanitizationReport::default();
            for item in items {
                let sanitized = sanitize_json_inner(item, depth + 1);
                report = report.merge(sanitized.report);
                out.push(sanitized.value);
            }
            Sanitized {
                value: Value::Array(out),
                report,
            }
        }
        Value::String(value) => {
            let sanitized = sanitize_text(value);
            Sanitized {
                value: Value::String(sanitized.value),
                report: sanitized.report,
            }
        }
        _ => Sanitized {
            value: value.clone(),
            report: SanitizationReport::default(),
        },
    }
}

/// True when a JSON object key's name itself suggests it holds a secret
/// (`api_key`, `token`, `password`, …), independent of the value's contents.
///
/// Matching keys are redacted wholesale in [`sanitize_json_inner`] — the
/// value is replaced rather than scanned, since a key named e.g. `password`
/// is assumed sensitive even if its value doesn't match any
/// [`REDACTION_PATTERNS`] regex. Matching is on the key with all
/// non-alphanumeric characters stripped and lowercased, so `API-Key`,
/// `api_key`, and `apiKey` are all treated identically.
fn is_sensitive_key(key: &str) -> bool {
    let normalized: String = key
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect();

    matches!(
        normalized.as_str(),
        "apikey"
            | "token"
            | "accesstoken"
            | "refreshtoken"
            | "authorization"
            | "password"
            | "secret"
            | "clientsecret"
    ) || normalized.ends_with("token")
        || normalized.ends_with("apikey")
        || normalized.ends_with("clientsecret")
        || normalized.contains("password")
        || normalized.contains("secret")
        || normalized.ends_with("key")
}

#[cfg(test)]
#[path = "safety_tests.rs"]
mod tests;
