use super::*;
use serde_json::json;

#[test]
fn sanitize_text_redacts_bearer_and_openai_key() {
    let input = "Authorization: Bearer abcdefghijklmnop and sk-1234567890123456789012345";
    let sanitized = sanitize_text(input);
    assert!(sanitized.value.contains("Bearer [REDACTED]"));
    assert!(!sanitized.value.contains("sk-1234567890123456789012345"));
    assert!(sanitized.report.text_redactions >= 2);
}

#[test]
fn sanitize_text_blocks_private_key_blocks() {
    let input = "-----BEGIN PRIVATE KEY-----\nabc\n-----END PRIVATE KEY-----";
    let sanitized = sanitize_text(input);
    assert!(sanitized.value.contains(REDACTED_PRIVATE_KEY));
    assert!(sanitized.report.blocked_secret_hits >= 1);
}

#[test]
fn sanitize_json_redacts_sensitive_keys_and_nested_strings() {
    let input = json!({
        "token": "abc123",
        "nested": {
            "notes": "Bearer supersecretvalue",
            "ok": "hello"
        },
        "arr": ["sk-1234567890123456789012345", "safe"]
    });

    let sanitized = sanitize_json(&input);
    assert_eq!(sanitized.value["token"], json!(REDACTED_SECRET));
    assert_eq!(sanitized.value["nested"]["ok"], json!("hello"));
    assert!(sanitized.value["nested"]["notes"]
        .as_str()
        .unwrap_or_default()
        .contains("[REDACTED]"));
    assert!(sanitized.report.key_redactions >= 1);
    assert!(sanitized.report.text_redactions >= 2);
}

#[test]
fn sanitize_json_redacts_common_sensitive_key_variants() {
    let input = json!({
        "db_password": "p@ss",
        "secret_key": "abc123",
        "api_secret": "def456",
        "monkey": "banana"
    });

    let sanitized = sanitize_json(&input);
    assert_eq!(sanitized.value["db_password"], json!(REDACTED_SECRET));
    assert_eq!(sanitized.value["secret_key"], json!(REDACTED_SECRET));
    assert_eq!(sanitized.value["api_secret"], json!(REDACTED_SECRET));
    assert_eq!(sanitized.value["monkey"], json!(REDACTED_SECRET));
    assert!(sanitized.report.key_redactions >= 4);
}

#[test]
fn has_likely_secret_detects_common_patterns() {
    assert!(has_likely_secret("api_key=abc123"));
    assert!(has_likely_secret("Bearer abcdefghijklmnopqrstuvwxyz"));
    assert!(has_likely_secret("xoxb-1234567890-abcdef-ghijklmnop"));
    assert!(has_likely_secret("glpat-aaaaaaaaaaaaaaaaaaaa"));
    assert!(has_likely_secret("SG.aaaaaaaaaaaaaaaa.bbbbbbbbbbbbbbbb"));
    assert!(!has_likely_secret("I prefer rust"));
}

#[test]
fn has_likely_pii_strict_boundary_flags_formatted_national_ids() {
    // The write-rejection boundary is the *strict* set: formatted national IDs
    // only. Bare-numeric / phone-shaped runs and email are excluded (too many
    // false positives against scanner-built identifiers); they are still
    // scrubbed by content redaction. Exhaustive coverage lives in `pii`'s tests.
    assert!(has_likely_pii("ssn 123-45-6789"));
    assert!(has_likely_pii("CPF 111.444.777-35"));
    assert!(has_likely_pii("cliente RFC VECJ880326XK4"));
    assert!(!has_likely_pii("call +15551234567")); // phone: content-scrub only
    assert!(!has_likely_pii("contact alice@example.com")); // email: out of scope
    assert!(!has_likely_pii("just a normal note"));
}

#[test]
fn sanitize_text_scrubs_pii_after_secrets() {
    let input = "Token sk-abcdefghijklmnopqrstuvwxyz; CPF 111.444.777-35; phone +15551234567";
    let sanitized = sanitize_text(input);
    assert!(!sanitized.value.contains("sk-abcdefghijklmnopqrstuvwxyz"));
    assert!(!sanitized.value.contains("111.444.777-35"));
    assert!(!sanitized.value.contains("+15551234567"));
    assert!(sanitized.report.pii_redactions >= 2);
}

#[test]
fn sanitize_json_redacts_values_beyond_max_depth() {
    let mut nested = json!("leaf");
    for _ in 0..(MAX_JSON_SANITIZE_DEPTH + 2) {
        nested = json!({ "nested": nested });
    }
    let sanitized = sanitize_json(&nested);
    assert!(sanitized.report.depth_redactions >= 1);
    assert!(sanitized
        .value
        .to_string()
        .contains(&format!("\"{REDACTED_SECRET}\"")));
}
