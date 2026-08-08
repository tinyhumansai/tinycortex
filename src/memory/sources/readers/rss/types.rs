//! Private feed-shape types for the RSS reader: the parsed entry model and
//! the short-lived cache snapshot reused across a list-then-read sync pass.

use std::time::Instant;

/// A fetched feed snapshot cached across a list-then-read sync pass.
pub struct FeedCache {
    pub url: String,
    pub fetched_at: Instant,
    pub entries: Vec<FeedEntry>,
}

/// One parsed RSS/Atom entry: the dedupe id, the fields surfaced as a source
/// item / content, and the raw link + publication timestamp carried in the
/// content metadata.
#[derive(Debug, Clone)]
pub struct FeedEntry {
    pub id: String,
    pub title: String,
    pub body: String,
    pub link: Option<String>,
    pub published: Option<String>,
    pub updated_at_ms: Option<i64>,
}
