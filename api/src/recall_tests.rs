//! Unit tests for the recall filter contracts in [`super`].
//!
//! The load-bearing test here is
//! `owned_and_borrowed_recall_opts_have_identical_fields`: it is the runtime
//! half of the field-parity defence described in the module docs (the compile
//! half being the exhaustive destructuring inside both `From` impls).

use super::*;
use serde_json::json;

/// Every field set to a non-default value, so a conversion that silently drops
/// one is visible.
fn fully_populated_owned() -> OwnedRecallOpts {
    OwnedRecallOpts {
        namespace: Some("projects".to_string()),
        category: Some(MemoryCategory::Custom("field_notes".to_string())),
        session_id: Some("session-42".to_string()),
        min_score: Some(0.75),
        cross_session: true,
    }
}

#[test]
fn owned_and_borrowed_recall_opts_have_identical_fields() {
    let owned = fully_populated_owned();

    // Owned → borrowed. Destructured exhaustively so a new field on
    // `RecallOpts` fails to compile here rather than silently going unchecked.
    let borrowed = RecallOpts::from(&owned);
    let RecallOpts {
        namespace,
        category,
        session_id,
        min_score,
        cross_session,
    } = borrowed.clone();
    assert_eq!(namespace, Some("projects"));
    assert_eq!(category, Some(MemoryCategory::Custom("field_notes".into())));
    assert_eq!(session_id, Some("session-42"));
    assert_eq!(min_score, Some(0.75));
    assert!(cross_session);

    // Borrowed → owned, and back to the value we started from. A field dropped
    // in either direction fails this equality.
    let round_tripped = OwnedRecallOpts::from(borrowed);
    assert_eq!(round_tripped, owned);
}

#[test]
fn borrowed_view_is_zero_copy_over_the_owned_strings() {
    let owned = fully_populated_owned();
    let borrowed = RecallOpts::from(&owned);

    // The borrowed form points *into* the owned value rather than at a copy;
    // that is the whole reason the borrowed form survives.
    assert_eq!(
        borrowed.namespace.unwrap().as_ptr(),
        owned.namespace.as_deref().unwrap().as_ptr()
    );
    assert_eq!(
        borrowed.session_id.unwrap().as_ptr(),
        owned.session_id.as_deref().unwrap().as_ptr()
    );
}

#[test]
fn owned_recall_opts_defaults_match_borrowed_defaults() {
    let owned = OwnedRecallOpts::default();
    let borrowed = RecallOpts::from(&owned);

    assert!(borrowed.namespace.is_none());
    assert!(borrowed.category.is_none());
    assert!(borrowed.session_id.is_none());
    assert!(borrowed.min_score.is_none());
    assert!(!borrowed.cross_session);

    // And the borrowed default converts back to the owned default.
    assert_eq!(OwnedRecallOpts::from(RecallOpts::default()), owned);
}

#[test]
fn owned_recall_opts_serde_round_trips_every_field() {
    let owned = fully_populated_owned();
    let encoded = serde_json::to_value(&owned).unwrap();

    assert_eq!(
        encoded,
        json!({
            "namespace": "projects",
            "category": "custom:field_notes",
            "session_id": "session-42",
            "min_score": 0.75,
            "cross_session": true
        })
    );

    let decoded: OwnedRecallOpts = serde_json::from_value(encoded).unwrap();
    assert_eq!(decoded, owned);
}

#[test]
fn empty_recall_body_deserializes_to_the_default() {
    // A minimal `POST /v1/memory/recall` body must be accepted: every field is
    // `#[serde(default)]`.
    let decoded: OwnedRecallOpts = serde_json::from_value(json!({})).unwrap();
    assert_eq!(decoded, OwnedRecallOpts::default());
}

#[test]
fn partial_recall_body_leaves_unmentioned_fields_at_default() {
    let decoded: OwnedRecallOpts =
        serde_json::from_value(json!({ "namespace": "global" })).unwrap();
    assert_eq!(decoded.namespace.as_deref(), Some("global"));
    assert!(decoded.category.is_none());
    assert!(decoded.session_id.is_none());
    assert!(decoded.min_score.is_none());
    assert!(!decoded.cross_session);
}

/// The wire form omits absent filters rather than emitting explicit nulls.
///
/// `OwnedRecallOpts` is the body of `POST /v1/memory/recall`, which the spec
/// describes as an optional-filters bag. Emitting `"namespace": null` for every
/// unset filter is valid JSON but forces a backend to distinguish "absent" from
/// "explicitly null" for no gain. Pinned here because changing the emitted shape
/// after a driver has shipped is observable to any backend that draws that
/// distinction.
#[test]
fn absent_recall_filters_are_omitted_from_the_wire_form() {
    let json = serde_json::to_value(OwnedRecallOpts::default()).expect("serialize");
    assert_eq!(
        json,
        serde_json::json!({ "cross_session": false }),
        "unset optional filters must be omitted, not serialized as null"
    );

    let populated = OwnedRecallOpts {
        namespace: Some("work".into()),
        ..Default::default()
    };
    let json = serde_json::to_value(&populated).expect("serialize");
    assert_eq!(
        json,
        serde_json::json!({ "namespace": "work", "cross_session": false })
    );
}

/// Omitting a filter and sending it as `null` must both decode to `None`, so a
/// backend built against either spelling keeps working.
#[test]
fn omitted_and_explicit_null_recall_filters_both_decode_to_none() {
    let omitted: OwnedRecallOpts = serde_json::from_str("{}").expect("decode {}");
    let explicit: OwnedRecallOpts =
        serde_json::from_str(r#"{"namespace":null,"category":null,"session_id":null,"min_score":null,"cross_session":false}"#)
            .expect("decode explicit nulls");
    assert_eq!(omitted, explicit);
    assert_eq!(omitted, OwnedRecallOpts::default());
}
