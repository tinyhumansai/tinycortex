use super::*;

#[test]
fn collapse_whitespace_normalizes() {
    assert_eq!(collapse_whitespace("  a\t b\n c  "), "a b c");
}

#[test]
fn collapse_whitespace_empty() {
    assert_eq!(collapse_whitespace("   "), "");
}

#[test]
fn normalize_search_text_lowercases_and_strips_special() {
    assert_eq!(normalize_search_text("Hello, WORLD!"), "hello world");
}

#[test]
fn normalize_search_text_preserves_separators() {
    assert_eq!(
        normalize_search_text("phoenix-project/v2.0"),
        "phoenix project v2 0"
    );
}

#[test]
fn normalize_graph_predicate_underscores_separators() {
    assert_eq!(normalize_graph_predicate("works on"), "WORKS_ON");
    assert_eq!(normalize_graph_predicate("due-on"), "DUE_ON");
}

#[test]
fn normalize_graph_predicate_strips_trailing_underscores() {
    assert_eq!(normalize_graph_predicate("  prefers  "), "PREFERS");
    assert_eq!(normalize_graph_predicate("a -- b"), "A_B");
}

#[test]
fn chunk_document_content_returns_nonempty_for_content() {
    let chunks = chunk_document_content("Hello world. This is a document.", 225);
    assert!(!chunks.is_empty());
}

#[test]
fn chunk_document_content_empty_input_returns_empty() {
    assert!(chunk_document_content("", 225).is_empty());
}

#[test]
fn chunk_document_content_whitespace_only_returns_empty() {
    assert!(chunk_document_content("   \n  ", 225).is_empty());
}
