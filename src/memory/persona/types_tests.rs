//! Tests for the persona evidence model (§6.3 contracts).

use super::*;
use chrono::TimeZone;

fn ts() -> DateTime<Utc> {
    Utc.timestamp_opt(1_700_000_000, 0).unwrap()
}

#[test]
fn evidence_ids_are_content_addressed_and_deterministic() {
    let src = EvidenceSource::new(PersonaSourceKind::ClaudeCode)
        .with_scope("proj")
        .with_session("sess-1");
    let a = PersonaEvidence::new(
        src.clone(),
        ts(),
        EvidenceTier::T2,
        "commit small and often",
        vec![],
    );
    let b = PersonaEvidence::new(
        src.clone(),
        ts(),
        EvidenceTier::T2,
        "commit small and often",
        vec![],
    );
    // Same source + same excerpt → same id (re-runs dedupe naturally).
    assert_eq!(a.id, b.id);
    assert_eq!(a.id.len(), 32);
    // Different excerpt → different id.
    let c = PersonaEvidence::new(src, ts(), EvidenceTier::T2, "different", vec![]);
    assert_ne!(a.id, c.id);
}

#[test]
fn evidence_is_redacted_at_construction() {
    let src = EvidenceSource::new(PersonaSourceKind::Codex);
    // A realistic transcript leak: an OpenAI-style API key. The composite
    // redactor scrubs secrets/tokens/keys before the excerpt is ever stored.
    let secret = "sk-abcdefghijklmnopqrstuvwxyz012345";
    let raw = format!("use this key {secret} to call the model");
    let ev = PersonaEvidence::new(src, ts(), EvidenceTier::T2, &raw, vec![]);
    // The secret must not survive into the stored excerpt.
    assert!(
        !ev.excerpt().contains(secret),
        "secret leaked into evidence excerpt: {}",
        ev.excerpt()
    );
    // The id is computed from the redacted excerpt, so it is independent of the
    // stripped PII: re-running with the already-redacted text yields the same id.
    let ev2 = PersonaEvidence::new(
        EvidenceSource::new(PersonaSourceKind::Codex),
        ts(),
        EvidenceTier::T2,
        ev.excerpt(),
        vec![],
    );
    assert_eq!(ev.id, ev2.id);
}

#[test]
fn tier_ordering_puts_t0_highest() {
    assert!(EvidenceTier::T0 > EvidenceTier::T1);
    assert!(EvidenceTier::T1 > EvidenceTier::T2);
    assert!(EvidenceTier::T2 > EvidenceTier::T3);
    assert_eq!(EvidenceTier::T0.rank(), 3);
    assert_eq!(EvidenceTier::T3.rank(), 0);
}

#[test]
fn tier_parses_loose_forms() {
    assert_eq!(EvidenceTier::parse_loose("T1"), Some(EvidenceTier::T1));
    assert_eq!(EvidenceTier::parse_loose(" tier2 "), Some(EvidenceTier::T2));
    assert_eq!(EvidenceTier::parse_loose("0"), Some(EvidenceTier::T0));
    assert_eq!(EvidenceTier::parse_loose("nonsense"), None);
}

#[test]
fn facet_parses_loose_forms_and_has_seven() {
    assert_eq!(PersonaFacet::ALL.len(), 7);
    assert_eq!(
        PersonaFacet::parse_loose("coding-style"),
        Some(PersonaFacet::CodingStyle)
    );
    assert_eq!(
        PersonaFacet::parse_loose("Anti Preferences"),
        Some(PersonaFacet::AntiPreferences)
    );
    assert_eq!(PersonaFacet::parse_loose("bogus"), None);
    // Directives leads the compile order (T0 budget-protected first).
    assert_eq!(PersonaFacet::ALL[0], PersonaFacet::Directives);
}

#[test]
fn facet_tree_scopes_are_namespaced_and_default_asks_present() {
    for f in PersonaFacet::ALL {
        assert_eq!(f.tree_scope(), format!("persona/{}", f.as_str()));
        assert!(!f.default_ask().trim().is_empty());
        assert!(!f.heading().trim().is_empty());
    }
}

#[test]
fn wire_strings_are_snake_case() {
    assert_eq!(PersonaSourceKind::ChatgptExport.as_str(), "chatgpt_export");
    assert_eq!(PersonaFacet::AntiPreferences.as_str(), "anti_preferences");
    // serde round-trips through the same encoding.
    let j = serde_json::to_string(&PersonaSourceKind::GitHistory).unwrap();
    assert_eq!(j, "\"git_history\"");
    let f: PersonaFacet = serde_json::from_str("\"coding_style\"").unwrap();
    assert_eq!(f, PersonaFacet::CodingStyle);
}

#[test]
fn session_digest_empty_helper() {
    let d = SessionDigest::empty(EvidenceSource::new(PersonaSourceKind::ClaudeCode));
    assert!(d.is_empty());
}
