//! Types for the web-page reader.

/// A parsed simple CSS selector: optional tag name, optional id, and class
/// names. Supports `tag`, `tag.class`, `tag#id`, `.class`, `#id`, and stacked
/// classes (`tag.a.b`, `.a.b`). A descendant/child chain (`div.content p`)
/// targets the final compound selector — a full CSS engine is out of scope for
/// this reader.
pub struct SelectorSpec {
    pub tag: Option<String>,
    pub id: Option<String>,
    pub classes: Vec<String>,
}
