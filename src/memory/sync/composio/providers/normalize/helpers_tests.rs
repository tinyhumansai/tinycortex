use super::*;
use serde_json::json;

#[test]
fn pick_str_finds_first_non_empty_match() {
    let v = json!({"data": {"user": {"name": "Ada", "email": "ada@example.com"}}});
    assert_eq!(
        pick_str(&v, &["data.user.name", "data.user.email"]),
        Some("Ada".into())
    );
    assert_eq!(
        pick_str(&v, &["data.missing", "data.user.email"]),
        Some("ada@example.com".into())
    );
    assert_eq!(pick_str(&v, &["nope.nope"]), None);
}

#[test]
fn pick_str_respects_path_order() {
    let v = json!({"a": "first", "b": "second"});
    assert_eq!(pick_str(&v, &["a", "b"]), Some("first".into()));
    assert_eq!(pick_str(&v, &["b", "a"]), Some("second".into()));
}

/// The drift guard for the divergence documented on [`pick_str`]. If this
/// ever starts returning `Some("42")`, someone has re-pointed the
/// normalisers at `common::pick_str` and changed their output.
#[test]
fn pick_str_rejects_non_string_values() {
    let v = json!({"count": 42, "flag": true, "empty": "", "whitespace": "   "});
    assert_eq!(pick_str(&v, &["count"]), None);
    assert_eq!(pick_str(&v, &["flag"]), None);
    assert_eq!(pick_str(&v, &["empty"]), None);
    assert_eq!(pick_str(&v, &["whitespace"]), None);
}
