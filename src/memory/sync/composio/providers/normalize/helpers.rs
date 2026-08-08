//! Shared helpers for the provider normalisers in this module.

/// Walk a JSON object using a list of dotted-path candidates and return the
/// first non-empty **string** match.
///
/// # This is deliberately NOT `super::super::common::pick_str`
///
/// The crate carries two `pick_str` functions with the same name and
/// genuinely different behaviour. Do not "deduplicate" them:
///
/// | | this one (`normalize::helpers`) | `common::pick_str` |
/// |---|---|---|
/// | traversal | `Value::get` per `.`-separated segment — objects only | `Value::pointer` — also indexes into arrays |
/// | non-string leaf | rejected, returns `None` | `Number` is coerced via `to_string()` |
///
/// The number case is the one that bites. A payload whose `id` is `42`
/// rather than `"42"` yields `None` here and `Some("42")` there, which
/// silently changes what a normaliser emits as a document id. The callers of
/// this function were written against the reject-non-strings behaviour and
/// have a test pinning it (`pick_str_rejects_non_string_values` below, and
/// the host-side mirror of it).
pub fn pick_str(value: &serde_json::Value, paths: &[&str]) -> Option<String> {
    for path in paths {
        let mut cur = value;
        let mut ok = true;
        for segment in path.split('.') {
            match cur.get(segment) {
                Some(next) => cur = next,
                None => {
                    ok = false;
                    break;
                }
            }
        }
        if !ok {
            continue;
        }
        if let Some(s) = cur.as_str() {
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
#[path = "helpers_tests.rs"]
mod tests;
