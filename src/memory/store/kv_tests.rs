use super::*;
use serde_json::json;

fn store() -> KvStore {
    KvStore::open_in_memory().unwrap()
}

#[test]
fn global_kv_roundtrips_and_deletes() {
    let kv = store();
    kv.set_global("theme", &json!("dark")).unwrap();
    assert_eq!(kv.get_global("theme").unwrap(), Some(json!("dark")));

    assert!(kv.delete_global("theme").unwrap());
    assert_eq!(kv.get_global("theme").unwrap(), None);
}

#[test]
fn namespace_kv_roundtrips_lists_and_combines_scope_records() {
    let kv = store();
    kv.set_global("global-setting", &json!(true)).unwrap();
    kv.set_namespace("team alpha/#1", "state", &json!({"open": true}))
        .unwrap();

    assert_eq!(
        kv.get_namespace("team alpha/#1", "state").unwrap(),
        Some(json!({"open": true}))
    );

    let listed = kv.list_namespace("team alpha/#1").unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["key"], "state");
    assert_eq!(listed[0]["value"], json!({"open": true}));

    let scoped = kv.records_for_scope("team alpha/#1").unwrap();
    assert_eq!(scoped.len(), 2);
    assert!(scoped
        .iter()
        .any(|r| r.namespace.is_none() && r.key == "global-setting"));
    assert!(scoped
        .iter()
        .any(|r| r.namespace.as_deref() == Some("team_alpha/_1") && r.key == "state"));
}

#[test]
fn namespace_is_sanitized_consistently_between_write_and_read() {
    let kv = store();
    kv.set_namespace("team alpha/#1", "k", &json!(1)).unwrap();
    // The canonical (already-sanitized) form addresses the same bucket.
    assert_eq!(
        kv.get_namespace("team_alpha/_1", "k").unwrap(),
        Some(json!(1))
    );
}

#[test]
fn kv_rejects_secret_like_keys() {
    let kv = store();
    let err = kv
        .set_global("sk-proj-abcdefghijklmnop", &json!("secret"))
        .unwrap_err();
    assert!(err.contains("cannot contain secrets"));

    let err = kv
        .set_namespace(
            "project",
            "ghp_abcdefghijklmnopqrstuvwx123456",
            &json!("secret"),
        )
        .unwrap_err();
    assert!(err.contains("cannot contain secrets"));
}

#[test]
fn kv_rejects_pii_like_keys() {
    let kv = store();
    let err = kv.set_global("alice@example.com", &json!("v")).unwrap_err();
    assert!(err.contains("personal identifiers"));
}

#[test]
fn kv_sanitizes_secret_values_before_storing() {
    let kv = store();
    kv.set_global(
        "notes",
        &json!("my key is sk-abcdefghijklmnopqrstuvwxyz1234"),
    )
    .unwrap();
    let stored = kv.get_global("notes").unwrap().unwrap();
    let s = stored.as_str().unwrap();
    assert!(!s.contains("sk-abcdefghijklmnopqrstuvwxyz1234"));
    assert!(s.contains("[REDACTED]"));
}

#[test]
fn missing_keys_return_none() {
    let kv = store();
    assert_eq!(kv.get_global("nope").unwrap(), None);
    assert_eq!(kv.get_namespace("ns", "nope").unwrap(), None);
    assert!(!kv.delete_global("nope").unwrap());
    assert!(!kv.delete_namespace("ns", "nope").unwrap());
}
