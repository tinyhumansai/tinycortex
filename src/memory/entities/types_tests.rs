//! Tests for [`super`] entity types. Ported from OpenHuman `memory_entities`.

use super::*;
use serde_json::json;

#[test]
fn entity_kind_roundtrips() {
    for kind in [
        EntityKind::Person,
        EntityKind::Organization,
        EntityKind::Topic,
        EntityKind::Email,
        EntityKind::Url,
        EntityKind::Handle,
        EntityKind::Hashtag,
        EntityKind::Location,
        EntityKind::Event,
        EntityKind::Product,
        EntityKind::Datetime,
        EntityKind::Technology,
        EntityKind::Artifact,
        EntityKind::Quantity,
        EntityKind::Misc,
    ] {
        assert_eq!(EntityKind::parse(kind.as_str()).unwrap(), kind);
    }
}

#[test]
fn entity_kind_parse_rejects_unknown() {
    assert!(EntityKind::parse("not-a-kind").is_err());
}

#[test]
fn entity_new_sets_empty_collections_and_timestamps() {
    let entity = Entity::new("person:alice", EntityKind::Person);
    assert_eq!(entity.id, "person:alice");
    assert_eq!(entity.kind, EntityKind::Person);
    assert!(entity.display_name.is_none());
    assert!(entity.aliases.is_empty());
    assert!(entity.emails.is_empty());
    assert!(entity.handles.is_empty());
    assert_eq!(entity.created_at, entity.updated_at);
}

#[test]
fn entity_handle_and_entity_serde_roundtrip() {
    let entity = Entity {
        id: "person:alice".into(),
        kind: EntityKind::Person,
        display_name: Some("Alice".into()),
        aliases: vec!["A".into()],
        emails: vec!["alice@example.com".into()],
        handles: vec![EntityHandle {
            kind: "slack".into(),
            value: "@alice".into(),
        }],
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };
    let value = serde_json::to_value(&entity).unwrap();
    assert_eq!(value["id"], json!("person:alice"));
    assert_eq!(value["kind"], json!("person"));
    assert_eq!(value["display_name"], json!("Alice"));

    let decoded: Entity = serde_json::from_value(value).unwrap();
    assert_eq!(decoded.id, entity.id);
    assert_eq!(decoded.kind, entity.kind);
    assert_eq!(decoded.display_name, entity.display_name);
    assert_eq!(decoded.aliases, entity.aliases);
    assert_eq!(decoded.emails, entity.emails);
    assert_eq!(decoded.handles, entity.handles);
}
