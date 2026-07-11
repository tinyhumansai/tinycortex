//! Tests for the hand-rolled YAML front-matter reader/writer.

use super::*;
use crate::memory::entities::types::{Entity, EntityHandle, EntityKind};

fn alice() -> Entity {
    let mut e = Entity::new("person:alice", EntityKind::Person);
    e.display_name = Some("Alice Cooper".into());
    e.aliases = vec!["Ali".into(), "A. Cooper".into()];
    e.emails = vec!["alice@example.com".into()];
    e.handles = vec![EntityHandle {
        kind: "slack".into(),
        value: "U12345".into(),
    }];
    e
}

#[test]
fn compose_parse_roundtrip() {
    let e = alice();
    let doc = compose(&e, "");
    let parsed = parse(&doc).expect("parses");
    assert_eq!(parsed.id, e.id);
    assert_eq!(parsed.kind, e.kind);
    assert_eq!(parsed.display_name, e.display_name);
    assert_eq!(parsed.aliases, e.aliases);
    assert_eq!(parsed.emails, e.emails);
    assert_eq!(parsed.handles, e.handles);
}

#[test]
fn compose_emits_expected_front_matter_shape() {
    let doc = compose(&alice(), "body text");
    assert!(doc.starts_with("---\nid: person:alice\nkind: person\n"));
    assert!(doc.contains("aliases:\n  - Ali\n  - A. Cooper\n"));
    assert!(doc.contains("emails:\n  - alice@example.com\n"));
    assert!(doc.contains("handles:\n  - kind: slack\n    value: U12345\n"));
    assert!(doc.contains("\n---\n\nbody text\n"));
}

#[test]
fn notes_body_returns_body_after_fence() {
    // The blank line separating the closing `---` fence from the body is part
    // of the preserved notes, so the body begins with a leading newline.
    let doc = compose(&alice(), "Met at the conference.");
    assert_eq!(
        notes_body(&doc).as_deref(),
        Some("\nMet at the conference.\n")
    );
}

#[test]
fn notes_body_none_without_front_matter() {
    assert_eq!(notes_body("no front matter here"), None);
}

#[test]
fn parse_accepts_closing_fence_at_eof() {
    // A hand-edited file whose closing `---` sits at end-of-file with no
    // trailing newline must still parse (regression: RS-14).
    let doc = "---\nid: person:alice\nkind: person\n---";
    let parsed = parse(doc).expect("EOF-fence document parses");
    assert_eq!(parsed.id, "person:alice");
    assert_eq!(parsed.kind, EntityKind::Person);
}

#[test]
fn notes_body_distinguishes_empty_body_from_unparsable() {
    // Recognizable front matter with an empty body → `Some("")`.
    let eof = "---\nid: person:alice\nkind: person\n---";
    assert_eq!(notes_body(eof).as_deref(), Some(""));
    // A body after the fence round-trips verbatim.
    let with_notes = "---\nid: person:alice\nkind: person\n---\n\nMet at conf.\n";
    assert_eq!(notes_body(with_notes).as_deref(), Some("\nMet at conf.\n"));
    // No recognizable front matter → `None`, so callers can refuse to clobber.
    assert_eq!(notes_body("free text with no fence\n"), None);
}

#[test]
fn parse_returns_none_without_kind() {
    let doc = "---\nid: person:x\n---\n\nbody\n";
    assert!(parse(doc).is_none());
}

#[test]
fn values_with_colons_round_trip_quoted() {
    let mut e = Entity::new("url:https://x.com/p", EntityKind::Url);
    e.display_name = Some("Home: page".into());
    e.handles = vec![EntityHandle {
        kind: "imessage".into(),
        value: "+1: 555".into(),
    }];
    let doc = compose(&e, "");
    let parsed = parse(&doc).expect("parses");
    assert_eq!(parsed.display_name.as_deref(), Some("Home: page"));
    assert_eq!(parsed.handles[0].value, "+1: 555");
}
