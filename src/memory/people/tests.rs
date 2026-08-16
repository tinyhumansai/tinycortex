//! Cross-file integration tests for the people domain.

use std::sync::Arc;

use chrono::Utc;

#[cfg(not(target_os = "macos"))]
use crate::memory::people::address_book;
use crate::memory::people::resolver::HandleResolver;
use crate::memory::people::store::PeopleStore;
use crate::memory::people::types::{Handle, PersonId};

/// Serialises every test that mutates the process-global people store.
///
/// The slot is process-wide, so two tests rebinding it concurrently race and
/// either may observe the other's store through `get()`.
static GLOBAL_STORE_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

#[tokio::test]
async fn resolver_and_store_cooperate_across_handle_kinds() {
    let s = PeopleStore::open_in_memory().unwrap();
    let r = HandleResolver::new(&s);

    // Email mints.
    let id = r
        .resolve_or_create(&Handle::Email("a@b.c".into()))
        .await
        .unwrap();
    // iMessage handle linked to same person.
    let id2 = r
        .link(
            &Handle::Email("a@b.c".into()),
            Handle::IMessage("+15551234".into()),
        )
        .await
        .unwrap();
    assert_eq!(id, id2);

    // Resolving by the linked iMessage handle returns the same id.
    let via_imsg = r
        .resolve(&Handle::IMessage("+15551234".into()))
        .await
        .unwrap();
    assert_eq!(via_imsg, Some(id));
}

#[cfg(not(target_os = "macos"))]
#[test]
fn address_book_is_empty_on_non_mac() {
    assert!(address_book::read().unwrap().is_empty());
}

/// Regression for Sentry TAURI-RUST-8NM (store never seeded → `get()` always
/// errored) and its #4378 follow-up (store stayed bound to the pre-login
/// workspace after an active-user switch). Verify `init_from_workspace` seeds
/// the global + creates the on-disk db, is an idempotent no-op for the same
/// workspace, and **rebinds** to a different workspace like `memory::global`.
///
/// Takes [`GLOBAL_STORE_LOCK`] because it mutates the process-global store
/// slot that any test may observe through `get()`. `#[test]` alone does **not**
/// serialise anything — the harness runs tests in parallel by default — so the
/// lock is what makes that true, and it must be taken by every future test that
/// touches the global.
#[test]
fn init_from_workspace_seeds_and_rebinds_global_store() {
    use crate::memory::people::store;

    let _serial = GLOBAL_STORE_LOCK.lock();

    let ws_a = tempfile::tempdir().unwrap();
    let store_a = store::init_from_workspace(ws_a.path()).unwrap();
    assert!(
        ws_a.path().join("people").join("people.db").exists(),
        "seed must create <workspace>/people/people.db"
    );

    // Previously-dead global is now reachable — the 8NM fix.
    let via_global = store::get().expect("people store reachable after seed");
    assert!(Arc::ptr_eq(&store_a, &via_global));

    // Same workspace → idempotent no-op, returns the same instance.
    let again = store::init_from_workspace(ws_a.path()).unwrap();
    assert!(Arc::ptr_eq(&store_a, &again));

    // Different workspace (active-user switch) → rebind to a new store. #4378.
    let ws_b = tempfile::tempdir().unwrap();
    let store_b = store::init_from_workspace(ws_b.path()).unwrap();
    assert!(
        !Arc::ptr_eq(&store_a, &store_b),
        "a new workspace must rebind to a fresh store, not reuse the old one"
    );
    let after_switch = store::get().expect("people store reachable after rebind");
    assert!(
        Arc::ptr_eq(&store_b, &after_switch),
        "get() must return the rebound (workspace B) store after a switch"
    );

    // Leave the global bound to a workspace that outlives this test. Both temp
    // dirs above are deleted when they drop, and the global would keep pointing
    // at whichever was bound last — so a later `get()` would hand back a store
    // over a database file that no longer exists. Leaking one directory is the
    // cheap way to keep the slot valid for the rest of the process; there is no
    // unbind, because production never unbinds either.
    let keep: &'static tempfile::TempDir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    store::init_from_workspace(keep.path()).unwrap();
}

#[test]
fn person_id_uuid_format() {
    let id = PersonId::new();
    // Round-trips through a string.
    let s = id.to_string();
    let parsed: uuid::Uuid = s.parse().unwrap();
    assert_eq!(parsed, id.0);
    let _now = Utc::now();
}
