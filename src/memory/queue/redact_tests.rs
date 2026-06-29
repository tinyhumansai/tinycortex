use super::*;

#[test]
fn passthrough_for_safe_string() {
    let input = "rate_limited: provider returned 429, retry in 30s";
    assert_eq!(scrub_for_log(input), input);
}

#[test]
fn masks_bearer_token() {
    let s = scrub_for_log("Authorization: Bearer abc123.def-456_xyz");
    assert!(s.contains("Bearer ***"), "got {s:?}");
    assert!(!s.contains("abc123"));
}

#[test]
fn masks_openai_key() {
    let s = scrub_for_log("upstream returned 401: invalid sk-abcDEF1234567890ZZZZ key");
    assert!(s.contains("sk-***"));
    assert!(!s.contains("sk-abcDEF1234567890ZZZZ"));
}

#[test]
fn masks_github_token_variants() {
    for raw in [
        "ghp_abcdefghij1234567890ABCD",
        "ghs_abcdefghij1234567890ABCD",
        "gho_abcdefghij1234567890ABCD",
    ] {
        let s = scrub_for_log(&format!("error: token {raw} rejected"));
        assert!(s.contains("***"), "input={raw} out={s}");
        assert!(!s.contains(raw), "input={raw} out={s}");
    }
}

#[test]
fn masks_slack_token() {
    let s = scrub_for_log("posting to slack failed: xoxb-1234567890-abcdEFG");
    assert!(s.contains("xox-***"));
    assert!(!s.contains("xoxb-1234567890"));
}

#[test]
fn masks_generic_secret_assignments() {
    let inputs = [
        ("api_key=hunter2 trailing", "api_key=***"),
        ("password: hunter2", "password=***"),
        ("Token = hunter2,more", "Token=***"),
        ("apiKey=\"hunter2\"", "apiKey=***"),
    ];
    for (raw, expect) in inputs {
        let s = scrub_for_log(raw);
        assert!(s.contains(expect), "input={raw:?} out={s:?}");
        assert!(!s.contains("hunter2"), "input={raw:?} out={s:?}");
    }
}

#[test]
fn strips_url_userinfo() {
    let s = scrub_for_log("connect failed https://alice:s3cret@db.internal/x");
    assert!(s.contains("https://***@db.internal/x"), "got {s:?}");
    assert!(!s.contains("alice"));
    assert!(!s.contains("s3cret"));
}

#[test]
fn masks_email() {
    let s = scrub_for_log("user alice@example.com triggered job");
    assert!(s.contains("***@***"), "got {s:?}");
    assert!(!s.contains("alice"));
    assert!(!s.contains("example.com"));
}

#[test]
fn truncates_oversized_input() {
    let big = "x".repeat(MAX_LEN * 2);
    let s = scrub_for_log(&big);
    assert!(s.len() < big.len());
    assert!(s.contains("(truncated,"));
}

#[test]
fn truncate_handles_multibyte_boundary() {
    let mut big = "a".repeat(MAX_LEN - 1);
    big.push('é');
    big.push_str(&"b".repeat(64));
    let s = scrub_for_log(&big);
    assert!(s.contains("(truncated,"));
}

#[test]
fn idempotent_on_already_scrubbed_string() {
    let once = scrub_for_log("Bearer abcdef api_key=hunter2");
    let twice = scrub_for_log(&once);
    assert_eq!(once, twice);
}
