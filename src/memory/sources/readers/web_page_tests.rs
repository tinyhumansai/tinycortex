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
fn extract_by_selector_preserves_case_for_ids_and_classes() {
    // CSS ids/classes are case-sensitive: `#Main` must match `id="Main"` and
    // `.ArticleBody` must match `class="ArticleBody"` rather than falling back
    // to whole-page extraction.
    let html = "<html><body><div id=\"Main\">Main text</div><p>Other</p></body></html>";
    let result = extract_by_selector(html, "#Main");
    assert!(result.contains("Main text"));
    assert!(!result.contains("Other"));

    let html =
        "<html><body><div class=\"ArticleBody\">Article text</div><p>Other</p></body></html>";
    let result = extract_by_selector(html, ".ArticleBody");
    assert!(result.contains("Article text"));
    assert!(!result.contains("Other"));

    // A selector in the wrong case must NOT match (id/class matching stays
    // case-sensitive): the result falls back to the whole stripped page, so
    // the unrelated sibling text leaks in — a targeted match would exclude it.
    let result = extract_by_selector(html, ".articlebody");
    assert!(result.contains("Other"));
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

#[test]
fn extract_by_selector_tolerates_unclosed_element() {
    // A page truncated mid-tag (a `<div` with no closing `>`) must not panic:
    // the unclosed element is skipped and extraction falls back to the
    // stripped page text.
    let html = "<html><body><article>Kept</article><div";
    let result = extract_by_selector(html, "div");
    assert!(result.contains("Kept"));
    assert!(!result.contains('<'));
}

// ── Attribute parsing ───────────────────────────────────────────────

#[test]
fn attr_value_reads_quoted_values() {
    // The second argument is the original-cased copy; for already-lowercase
    // input both are the same string.
    assert_eq!(
        attr_value("<div id=\"a\">", "<div id=\"a\">", "id").as_deref(),
        Some("a")
    );
    assert_eq!(
        attr_value("<div id='b'>", "<div id='b'>", "id").as_deref(),
        Some("b")
    );
    assert_eq!(
        attr_value("<div class=\"x y\">", "<div class=\"x y\">", "class").as_deref(),
        Some("x y")
    );
    assert_eq!(attr_value("<div>", "<div>", "id"), None);
}

#[test]
fn attr_value_does_not_match_word_prefix() {
    // `classy=` must not be read as the `class` attribute.
    assert_eq!(
        attr_value("<div classy=\"z\">", "<div classy=\"z\">", "class"),
        None
    );
}

#[test]
fn attr_value_preserves_original_case() {
    // The scan runs on the lowercased tag, but the value is read from the
    // original-cased copy so case-sensitive CSS ids/classes still match.
    assert_eq!(
        attr_value("<div id=\"main\">", "<div id=\"Main\">", "id").as_deref(),
        Some("Main")
    );
    assert_eq!(
        attr_value(
            "<div class=\"articlebody\">",
            "<div class=\"ArticleBody\">",
            "class"
        )
        .as_deref(),
        Some("ArticleBody")
    );
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
