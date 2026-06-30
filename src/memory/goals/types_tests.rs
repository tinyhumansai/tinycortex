//! Unit tests for [`super::GoalsDoc`] parse/render and in-memory mutation.
//! Ported from OpenHuman `memory_goals/types.rs`.

use super::*;

#[test]
fn parse_round_trips_render() {
    let mut doc = GoalsDoc::default();
    doc.add("ship the desktop app").unwrap();
    doc.add("keep the rust core authoritative").unwrap();
    let rendered = doc.render();
    let reparsed = GoalsDoc::parse(&rendered);
    assert_eq!(doc, reparsed);
}

#[test]
fn render_starts_with_header() {
    let doc = GoalsDoc::default();
    assert!(doc.render().starts_with("# Long-term Goals"));
}

#[test]
fn parse_ignores_non_item_lines() {
    let body = "# Long-term Goals\n\nsome stray prose\n- [g1] real goal\n- malformed line\n";
    let doc = GoalsDoc::parse(body);
    assert_eq!(doc.items.len(), 1);
    assert_eq!(doc.items[0].id, "g1");
    assert_eq!(doc.items[0].text, "real goal");
}

#[test]
fn add_assigns_unique_ids() {
    let mut doc = GoalsDoc::default();
    let a = doc.add("a").unwrap();
    let b = doc.add("b").unwrap();
    assert_ne!(a, b);
    assert_eq!(doc.items.len(), 2);
}

#[test]
fn add_rejects_empty_text() {
    let mut doc = GoalsDoc::default();
    assert!(doc.add("   ").is_err());
}

#[test]
fn add_and_edit_reject_multiline_text() {
    let mut doc = GoalsDoc::default();
    // A newline-bearing goal would inject extra "- [..]" list lines on reload,
    // corrupting the stored shape — reject it outright.
    assert!(doc.add("line one\n- [x] injected").is_err());
    let id = doc.add("legit goal").unwrap();
    assert!(doc.edit(&id, "still\rinjected").is_err());
}

#[test]
fn add_and_edit_reject_secret_or_pii_text() {
    let mut doc = GoalsDoc::default();
    assert!(doc
        .add("follow up with alice@example.com about launch")
        .is_err());
    assert!(doc
        .add("rotate api_key=sk-abcdefghijklmnopqrstuvwxyz123456")
        .is_err());

    let id = doc.add("ship the memory engine").unwrap();
    assert!(doc.edit(&id, "call +14155551212 tomorrow").is_err());
    assert_eq!(doc.items[0].text, "ship the memory engine");
}

#[test]
fn edit_updates_known_id_and_rejects_unknown() {
    let mut doc = GoalsDoc::default();
    let id = doc.add("old").unwrap();
    doc.edit(&id, "new").unwrap();
    assert_eq!(doc.items[0].text, "new");
    assert!(doc.edit("nope", "x").is_err());
}

#[test]
fn delete_removes_known_id_and_rejects_unknown() {
    let mut doc = GoalsDoc::default();
    let id = doc.add("x").unwrap();
    doc.delete(&id).unwrap();
    assert!(doc.is_empty());
    assert!(doc.delete("nope").is_err());
}

#[test]
fn next_id_avoids_collision_with_custom_ids() {
    let mut doc = GoalsDoc {
        items: vec![GoalItem::new("g1", "a"), GoalItem::new("g2", "b")],
    };
    let id = doc.add("c").unwrap();
    assert_eq!(id, "g3");
}
