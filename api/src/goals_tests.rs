//! Unit tests for [`super::GoalsDoc`] parse/render — the pure-data half.
//!
//! The validating mutation tests (`add` / `edit` / `delete`, including the
//! secret/PII rejection cases) live in the engine crate next to the
//! `GoalsDocMutations` trait that owns them: `memory::goals::mutations_tests`.

use super::*;

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
