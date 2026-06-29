//! Canonical id derivation and filename slugging.
//!
//! Canonicalisation is exact-match only: it normalises surface forms
//! (lowercase emails, strip a leading `@` on handles, strip a leading `#` on
//! topics/hashtags) and assigns a deterministic, stable canonical id of the
//! form `<kind>:<normalised-surface>`. The same surface form always maps to
//! the same id regardless of casing or decoration, so cross-source mentions
//! of the same thing collapse onto one registry file.
//!
//! Fuzzy matching (e.g. `alice-slack` ≡ `Alice-Discord` by soft match) is out
//! of scope here; the mechanical cases are handled cleanly without producing
//! false merges.

use super::types::EntityKind;

/// Canonical id form per kind. Deterministic so the same surface always maps
/// to the same id.
///
/// - Email: `email:<lowercased>`
/// - Handle: `handle:<lowercased>` with a leading `@` stripped
/// - Hashtag: `hashtag:<lowercased>` with a leading `#` stripped
/// - Topic: `topic:<lowercased>` with leading `@`/`#` stripped
/// - URL: `url:<trimmed>` with case preserved for path/query exact matching
/// - Other kinds: `<kind>:<lowercased-surface>`
///
/// URLs keep their original case because path and query components are
/// case-significant; every other kind is folded to lowercase so casing never
/// fragments an identity.
pub fn canonical_id_for(kind: EntityKind, surface: &str) -> String {
    let trimmed = surface.trim();
    let clean = if kind == EntityKind::Url {
        trimmed.to_string()
    } else {
        trimmed
            .to_lowercase()
            .trim_start_matches('@')
            .trim_start_matches('#')
            .to_string()
    };
    format!("{}:{}", kind.as_str(), clean)
}

/// Map a canonical id to a filesystem-safe filename stem.
///
/// `:` is replaced (along with the other Windows-reserved characters and
/// control bytes) so the same on-disk layout works on every platform, even
/// though `:` is legal on Unix. The authoritative id always lives in the
/// file's YAML `id:` field, so the slug is only a content-addressed handle —
/// the parser never reconstructs the id from the filename.
pub(crate) fn slugify_id(id: &str) -> String {
    id.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect()
}

#[cfg(test)]
#[path = "canonical_tests.rs"]
mod tests;
