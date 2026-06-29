//! Persistence + cap + path-safety tests for [`super::store`]. Ported from
//! OpenHuman `memory_goals/store.rs`, plus the symlink-escape rejection
//! required by the TinyCortex port.

use super::*;
use crate::memory::error::MemoryError;

#[test]
fn load_empty_when_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let doc = load(tmp.path()).unwrap();
    assert!(doc.is_empty());
}

#[test]
fn add_edit_delete_round_trip_to_disk() {
    let tmp = tempfile::tempdir().unwrap();
    let (id, _) = add(tmp.path(), "ship the app").unwrap();

    let reloaded = load(tmp.path()).unwrap();
    assert_eq!(reloaded.items.len(), 1);
    assert_eq!(reloaded.items[0].text, "ship the app");

    edit(tmp.path(), &id, "ship the app to all platforms").unwrap();
    let reloaded = load(tmp.path()).unwrap();
    assert_eq!(reloaded.items[0].text, "ship the app to all platforms");

    delete(tmp.path(), &id).unwrap();
    let reloaded = load(tmp.path()).unwrap();
    assert!(reloaded.is_empty());
}

#[test]
fn save_enforces_item_count_cap() {
    let tmp = tempfile::tempdir().unwrap();
    let mut doc = GoalsDoc::default();
    for i in 0..(GOALS_MAX_ITEMS + 3) {
        doc.add(&format!("goal number {i}")).unwrap();
    }
    save(tmp.path(), &mut doc).unwrap();
    assert_eq!(doc.items.len(), GOALS_MAX_ITEMS);
    // The oldest items (goal number 0..2) should have been dropped.
    assert!(doc.items.iter().all(|i| i.text != "goal number 0"));
}

#[test]
fn save_enforces_byte_cap() {
    let tmp = tempfile::tempdir().unwrap();
    let mut doc = GoalsDoc::default();
    // Two large items that together exceed the byte cap.
    let big = "x".repeat(GOALS_FILE_MAX_CHARS);
    doc.add(&big).unwrap();
    doc.add(&big).unwrap();
    save(tmp.path(), &mut doc).unwrap();
    // At least one item dropped; never fully emptied.
    assert_eq!(doc.items.len(), 1);
    // The persisted file must respect the byte cap is loosely held: a single
    // oversized entry is allowed, but two are not.
    assert!(doc.render().len() <= GOALS_FILE_MAX_CHARS + big.len());
}

#[test]
fn config_rooted_wrappers_round_trip() {
    use crate::memory::config::MemoryConfig;
    let tmp = tempfile::tempdir().unwrap();
    let cfg = MemoryConfig::new(tmp.path());

    let (id, doc) = add_for(&cfg, "learn rust").unwrap();
    assert_eq!(doc.items.len(), 1);
    let doc = edit_for(&cfg, &id, "learn rust deeply").unwrap();
    assert_eq!(doc.items[0].text, "learn rust deeply");
    let listed = list_for(&cfg).unwrap();
    assert_eq!(listed.items.len(), 1);
    let doc = delete_for(&cfg, &id).unwrap();
    assert!(doc.is_empty());
}

#[cfg(unix)]
#[test]
fn rejects_symlink_escape_outside_workspace() {
    use std::os::unix::fs::symlink;

    let workspace = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();

    // A real target file living OUTSIDE the workspace.
    let evil_target = outside.path().join("evil.md");
    std::fs::write(&evil_target, "# Long-term Goals\n\n- [g1] exfiltrated\n").unwrap();

    // MEMORY_GOALS.md inside the workspace is a symlink pointing at it.
    let link = goals_path(workspace.path());
    symlink(&evil_target, &link).unwrap();

    // Both reads and writes must refuse the escaping link.
    let load_err = load(workspace.path()).unwrap_err();
    assert!(matches!(load_err, MemoryError::PathEscape(_)));

    let mut doc = GoalsDoc::default();
    doc.add("benign").unwrap();
    let save_err = save(workspace.path(), &mut doc).unwrap_err();
    assert!(matches!(save_err, MemoryError::PathEscape(_)));

    // The escape target must be untouched.
    let target_body = std::fs::read_to_string(&evil_target).unwrap();
    assert!(target_body.contains("exfiltrated"));
}
