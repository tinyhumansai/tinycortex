use super::*;

#[test]
fn strip_html_tags_removes_tags() {
    let html = "<p>Hello <b>world</b></p>";
    assert_eq!(strip_html_tags(html), "Hello world");
}

#[test]
fn extract_title_finds_title_tag() {
    let html = "<html><head><title>My Page</title></head><body></body></html>";
    assert_eq!(extract_title(html).as_deref(), Some("My Page"));
}

#[test]
fn extract_by_selector_finds_tag_content() {
    let html = "<html><body><article><p>Important content</p></article><footer>skip</footer></body></html>";
    let result = extract_by_selector(html, "article");
    assert!(result.contains("Important content"));
    assert!(!result.contains("skip"));
}

#[test]
fn extract_by_selector_fallback_on_missing_tag() {
    let html = "<html><body>All the text</body></html>";
    let result = extract_by_selector(html, "article");
    assert!(result.contains("All the text"));
}

// ── Selector parsing ────────────────────────────────────────────────

#[test]
fn parse_selector_supports_class_only() {
    let spec = parse_selector(".content").unwrap();
    assert_eq!(spec.tag, None);
    assert_eq!(spec.id, None);
    assert_eq!(spec.classes, vec![String::from("content")]);
}

#[test]
fn parse_selector_supports_id() {
    let spec = parse_selector("#main").unwrap();
    assert_eq!(spec.tag, None);
    assert_eq!(spec.id.as_deref(), Some("main"));
    assert!(spec.classes.is_empty());
}

#[test]
fn parse_selector_supports_tag_class_and_id() {
    let spec = parse_selector("div#content.wide").unwrap();
    assert_eq!(spec.tag.as_deref(), Some("div"));
    assert_eq!(spec.id.as_deref(), Some("content"));
    assert_eq!(spec.classes, vec![String::from("wide")]);
}

#[test]
fn parse_selector_supports_stacked_classes() {
    let spec = parse_selector(".a.b.c").unwrap();
    assert_eq!(
        spec.classes,
        vec![String::from("a"), String::from("b"), String::from("c")]
    );
}

#[test]
fn parse_selector_targets_last_compound_of_descendant() {
    // A full CSS engine is out of scope; a descendant chain selects the final
    // compound selector so `div.content p` still matches the paragraphs.
    let spec = parse_selector("div.content p").unwrap();
    assert_eq!(spec.tag.as_deref(), Some("p"));
    assert!(spec.classes.is_empty());
}

#[test]
fn parse_selector_rejects_garbage() {
    assert!(parse_selector("").is_none());
    assert!(parse_selector("   ").is_none());
    assert!(parse_selector("#").is_none());
    assert!(parse_selector(".").is_none());
}

// ── Selector extraction ─────────────────────────────────────────────

#[test]
fn extract_by_selector_matches_class() {
    let html = "<html><body><div class=\"content\">Kept</div><div>Dropped</div></body></html>";
    let result = extract_by_selector(html, ".content");
    assert!(result.contains("Kept"));
    assert!(!result.contains("Dropped"));
}

#[test]
fn extract_by_selector_matches_id() {
    let html = "<html><body><main id=\"main\">Main body</main><aside>Sidebar</aside></body></html>";
    let result = extract_by_selector(html, "#main");
    assert!(result.contains("Main body"));
    assert!(!result.contains("Sidebar"));
}

#[test]
fn extract_by_selector_matches_tag_class_compound() {
    let html =
        "<html><body><div class=\"card\">Card</div><span class=\"card\">Span</span></body></html>";
    let result = extract_by_selector(html, "div.card");
    assert!(result.contains("Card"));
    assert!(!result.contains("Span"));
}

#[test]
fn extract_by_selector_requires_all_stacked_classes() {
    let html = "<html><body><p class=\"a\">Only A</p><p class=\"a b\">Both</p></body></html>";
    let result = extract_by_selector(html, ".a.b");
    assert!(result.contains("Both"));
    assert!(!result.contains("Only A"));
}

#[test]
fn extract_by_selector_handles_nested_same_tag() {
    let html = "<html><body><div class=\"outer\"><div class=\"inner\">Inner</div> Outer</div></body></html>";
    let result = extract_by_selector(html, ".outer");
    assert!(result.contains("Inner"));
    assert!(result.contains("Outer"));
}

#[test]
fn extract_by_selector_falls_back_when_class_missing() {
    let html = "<html><body>Fallback text</body></html>";
    let result = extract_by_selector(html, ".nope");
    assert!(result.contains("Fallback text"));
}

// ── Attribute parsing ───────────────────────────────────────────────

#[test]
fn attr_value_reads_quoted_values() {
    assert_eq!(attr_value("<div id=\"a\">", "id").as_deref(), Some("a"));
    assert_eq!(attr_value("<div id='b'>", "id").as_deref(), Some("b"));
    assert_eq!(
        attr_value("<div class=\"x y\">", "class").as_deref(),
        Some("x y")
    );
    assert_eq!(attr_value("<div>", "id"), None);
}

#[test]
fn attr_value_does_not_match_word_prefix() {
    // `classy=` must not be read as the `class` attribute.
    assert_eq!(attr_value("<div classy=\"z\">", "class"), None);
}

#[test]
fn tag_name_reads_leading_identifier() {
    assert_eq!(tag_name("<div class=\"x\">"), Some("div"));
    assert_eq!(tag_name("<my-tag id=\"a\">"), Some("my-tag"));
    assert_eq!(tag_name(">"), None);
}

// ── script/style stripping ──────────────────────────────────────────

#[test]
fn strip_html_tags_removes_script_and_style_bodies() {
    let html = "<p>Hello</p><script>var x = '<b>not text</b>';</script><style>.x{color:red}</style><p>World</p>";
    let result = strip_html_tags(html);
    assert_eq!(result, "Hello World");
}

#[test]
fn strip_script_and_style_handles_unclosed() {
    // Unclosed `<script>` at the end of the page: the scan must not panic and
    // must drop the trailing (broken) script body rather than leak it.
    let html = "<p>a</p><script>never closed";
    let result = strip_script_and_style(html);
    assert_eq!(result, "<p>a</p>");
}

#[test]
fn extract_by_selector_ignores_script_bodies() {
    let html = "<html><body><div class=\"content\">Real</div><script>document.write('<div class=\"content\">Fake</div>');</script></body></html>";
    let result = extract_by_selector(html, ".content");
    assert!(result.contains("Real"));
    assert!(!result.contains("Fake"));
}
