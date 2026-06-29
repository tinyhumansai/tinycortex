//! Tests for the disk-backed entity store. Ported from OpenHuman
//! `memory_entities::store`, including the upsert-preserves-notes invariant.

use super::*;
use crate::memory::config::MemoryConfig;
use crate::memory::entities::types::{Entity, EntityHandle, EntityKind};
use tempfile::TempDir;

fn cfg() -> (TempDir, MemoryConfig) {
    let tmp = TempDir::new().unwrap();
    let config = MemoryConfig::new(tmp.path());
    (tmp, config)
}

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
fn round_trip_person() {
    let (_t, c) = cfg();
    let stored = put_entity(&c, alice()).unwrap();
    let got = get_entity(&c, EntityKind::Person, "person:alice")
        .unwrap()
        .expect("entity present");
    assert_eq!(got.id, stored.id);
    assert_eq!(got.display_name.as_deref(), Some("Alice Cooper"));
    assert_eq!(got.aliases, vec!["Ali".to_string(), "A. Cooper".into()]);
    assert_eq!(got.emails, vec!["alice@example.com".to_string()]);
    assert_eq!(got.handles.len(), 1);
    assert_eq!(got.handles[0].kind, "slack");
    assert_eq!(got.handles[0].value, "U12345");
}

#[test]
fn missing_entity_returns_none() {
    let (_t, c) = cfg();
    assert!(get_entity(&c, EntityKind::Person, "person:nope")
        .unwrap()
        .is_none());
}

#[test]
fn list_entities_by_kind() {
    let (_t, c) = cfg();
    put_entity(&c, alice()).unwrap();
    let mut bob = Entity::new("person:bob", EntityKind::Person);
    bob.display_name = Some("Bob".into());
    put_entity(&c, bob).unwrap();
    let mut org = Entity::new("organization:acme", EntityKind::Organization);
    org.display_name = Some("Acme".into());
    put_entity(&c, org).unwrap();

    let people = list_entities(&c, EntityKind::Person).unwrap();
    assert_eq!(people.len(), 2);
    let orgs = list_entities(&c, EntityKind::Organization).unwrap();
    assert_eq!(orgs.len(), 1);
    assert_eq!(orgs[0].display_name.as_deref(), Some("Acme"));
}

#[test]
fn lookup_alias_finds_by_alias_email_handle_or_name() {
    let (_t, c) = cfg();
    put_entity(&c, alice()).unwrap();
    assert_eq!(
        lookup_alias(&c, EntityKind::Person, "Ali")
            .unwrap()
            .unwrap()
            .id,
        "person:alice"
    );
    assert_eq!(
        lookup_alias(&c, EntityKind::Person, "alice@example.com")
            .unwrap()
            .unwrap()
            .id,
        "person:alice"
    );
    assert_eq!(
        lookup_alias(&c, EntityKind::Person, "U12345")
            .unwrap()
            .unwrap()
            .id,
        "person:alice"
    );
    assert_eq!(
        lookup_alias(&c, EntityKind::Person, "alice cooper")
            .unwrap()
            .unwrap()
            .id,
        "person:alice"
    );
    assert!(lookup_alias(&c, EntityKind::Person, "noone")
        .unwrap()
        .is_none());
}

#[test]
fn upsert_preserves_user_notes_body() {
    let (_t, c) = cfg();
    put_entity(&c, alice()).unwrap();
    // User hand-edits the file to add notes.
    let path = entity_path(&c, EntityKind::Person, "person:alice");
    let original = fs::read_to_string(&path).unwrap();
    let with_notes = format!("{original}\nMet at the conference in March.\n");
    fs::write(&path, &with_notes).unwrap();

    // Re-upsert with a new alias — notes should survive.
    let mut updated = alice();
    updated.aliases.push("Coop".into());
    put_entity(&c, updated).unwrap();

    let body = fs::read_to_string(&path).unwrap();
    assert!(body.contains("Met at the conference in March."));
    assert!(body.contains("Coop"));
}

#[test]
fn entity_file_lands_at_expected_path() {
    let (_t, c) = cfg();
    put_entity(&c, alice()).unwrap();
    let path = entity_path(&c, EntityKind::Person, "person:alice");
    assert!(path.ends_with("memory_tree/content/entities/person/person_alice.md"));
    assert!(path.exists());
}
