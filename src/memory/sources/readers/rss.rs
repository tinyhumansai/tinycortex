//! RSS/Atom feed source reader.
//!
//! Fetches and parses an RSS or Atom feed, returning entries as
//! source items. Uses a lightweight XML parser (`quick-xml` via
//! manual parsing) to avoid pulling in heavy feed crates.
//!
//! Fetches go through the shared `ssrf` guard (scheme/host policy, a DNS
//! resolver that pins connections to globally routable addresses, and
//! per-hop redirect re-checks), and the parsed feed is cached briefly so a
//! list-then-read sync pass downloads it once rather than once per entry.

mod types;

use std::sync::Mutex;
use std::time::{Duration, Instant};

use async_trait::async_trait;

use crate::memory::config::MemoryConfig;
use crate::memory::error::MemoryEngineResult;
use crate::memory::sources::types::{
    ContentType, MemorySourceEntry, SourceContent, SourceItem, SourceKind,
};

use super::ssrf::{build_client, is_url_allowed, read_body_capped};
use super::{into_engine_error, SourceReader};
use types::{FeedCache, FeedEntry};

const DEFAULT_MAX_ITEMS: u32 = 50;
const MAX_FEED_BYTES: u64 = 5 * 1024 * 1024; // 5 MiB — guards against pathological feeds

/// How long a fetched feed is reused before the next read re-downloads it.
///
/// Kept short so a feed that updates mid-sync is picked up on the next sync;
/// long enough to cover a list-then-read pass over a 50-entry feed.
const FEED_CACHE_TTL: Duration = Duration::from_secs(60);

pub struct RssReader {
    cache: Mutex<Option<FeedCache>>,
}

impl RssReader {
    pub fn new() -> Self {
        Self::default()
    }

    /// Fetch (or reuse a very fresh copy of) the feed at `url`.
    ///
    /// The workspace sync pipeline holds one reader across a tick and calls
    /// `list_items` once, then `read_item` once per entry. Without a cache
    /// that is N+1 downloads of the same feed per sync (and a rate-limit
    /// risk against the feed host); the cache turns it into one fetch whose
    /// results are reused for the read phase.
    async fn fetch_entries(&self, url: &str) -> Result<Vec<FeedEntry>, String> {
        // Read the cache in a nested scope so the mutex guard is dropped before
        // the await below — the guard is not `Send`, and holding it across an
        // await would make the reader's async methods non-`Send`.
        {
            let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(cached) = cache.as_ref() {
                if cached.url == url && cached.fetched_at.elapsed() < FEED_CACHE_TTL {
                    return Ok(cached.entries.clone());
                }
            }
        }

        let body = fetch_url(url).await?;
        let entries = parse_feed_full(&body)?;
        *self.cache.lock().unwrap_or_else(|e| e.into_inner()) = Some(FeedCache {
            url: url.to_string(),
            fetched_at: Instant::now(),
            entries: entries.clone(),
        });
        Ok(entries)
    }
}

impl Default for RssReader {
    fn default() -> Self {
        Self {
            cache: Mutex::new(None),
        }
    }
}

#[async_trait]
impl SourceReader for RssReader {
    fn kind(&self) -> SourceKind {
        SourceKind::RssFeed
    }

    async fn list_items(
        &self,
        source: &MemorySourceEntry,
        config: &MemoryConfig,
    ) -> MemoryEngineResult<Vec<SourceItem>> {
        self.list_items_inner(source, config)
            .await
            .map_err(into_engine_error)
    }

    async fn read_item(
        &self,
        source: &MemorySourceEntry,
        item_id: &str,
        config: &MemoryConfig,
    ) -> MemoryEngineResult<SourceContent> {
        self.read_item_inner(source, item_id, config)
            .await
            .map_err(into_engine_error)
    }
}

impl RssReader {
    async fn list_items_inner(
        &self,
        source: &MemorySourceEntry,
        _config: &MemoryConfig,
    ) -> Result<Vec<SourceItem>, String> {
        let url = source.url.as_deref().ok_or("rss source requires a url")?;
        let max_items = source.max_items.unwrap_or(DEFAULT_MAX_ITEMS) as usize;

        tracing::debug!(
            host = %url_host(url),
            max_items = max_items,
            "[memory_sources:rss] listing items"
        );

        let entries = self.fetch_entries(url).await?;

        tracing::debug!(count = entries.len(), "[memory_sources:rss] parsed entries");

        Ok(entries
            .into_iter()
            .take(max_items)
            .map(|e| SourceItem {
                id: e.id,
                title: e.title,
                updated_at_ms: e.updated_at_ms,
            })
            .collect())
    }

    async fn read_item_inner(
        &self,
        source: &MemorySourceEntry,
        item_id: &str,
        _config: &MemoryConfig,
    ) -> Result<SourceContent, String> {
        let url = source.url.as_deref().ok_or("rss source requires a url")?;

        tracing::debug!(
            host = %url_host(url),
            item_id = %item_id,
            "[memory_sources:rss] reading item"
        );

        let entries = self.fetch_entries(url).await?;
        let entry = entries
            .into_iter()
            .find(|e| e.id == item_id)
            .ok_or_else(|| format!("item '{item_id}' not found in feed"))?;

        let content_type = if entry.body.contains('<') {
            ContentType::Html
        } else {
            ContentType::Plaintext
        };

        Ok(SourceContent {
            id: entry.id,
            title: entry.title,
            body: entry.body,
            content_type,
            metadata: serde_json::json!({
                "link": entry.link,
                "published": entry.published,
            }),
        })
    }
}

/// Extract just the host portion of a URL for debug-log redaction so we
/// don't leak query params, paths, or embedded credentials (userinfo).
fn url_host(url: &str) -> String {
    // A real parse drops any `user:pass@` prefix via `host_str()`. When the
    // value is not a parseable URL (it will be rejected by the SSRF guard
    // later anyway), fall back to a textual host extraction that still strips
    // userinfo and the path/query/fragment.
    reqwest::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| {
            let authority = url
                .trim_start_matches("https://")
                .trim_start_matches("http://")
                .split(['/', '?', '#'])
                .next()
                .unwrap_or(url);
            // Only the last `@`-separated segment can be the host; anything
            // before it is credentials and must not reach the log.
            authority
                .rsplit('@')
                .next()
                .unwrap_or(authority)
                .to_string()
        })
}

async fn fetch_url(url: &str) -> Result<String, String> {
    // SSRF guard: validate scheme and host, reject private/internal targets,
    // and refuse redirects that would escape that policy.
    let parsed = reqwest::Url::parse(url).map_err(|e| format!("invalid URL: {e}"))?;
    if !is_url_allowed(&parsed) {
        return Err(format!(
            "rss source requires an http(s) URL to a public host, got: {}",
            url.chars().take(64).collect::<String>()
        ));
    }

    let client = build_client()?;
    let resp = client
        .get(parsed)
        .header("User-Agent", "openhuman")
        .send()
        .await
        .map_err(|e| format!("failed to fetch feed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("feed returned {}", resp.status()));
    }

    // Stream the body with a cap so a pathological feed can't OOM us before
    // the size check runs (`Content-Length` can be omitted or understated).
    let bytes = read_body_capped(resp, MAX_FEED_BYTES).await?;
    String::from_utf8(bytes).map_err(|e| format!("feed body is not valid UTF-8: {e}"))
}

fn parse_feed_full(xml: &str) -> Result<Vec<FeedEntry>, String> {
    // Detect RSS vs Atom by looking for <rss or <feed
    if xml.contains("<rss") || xml.contains("<channel") {
        parse_rss(xml)
    } else if xml.contains("<feed") {
        parse_atom(xml)
    } else {
        Err("unrecognized feed format (expected RSS or Atom)".to_string())
    }
}

fn parse_rss(xml: &str) -> Result<Vec<FeedEntry>, String> {
    let mut entries = Vec::new();
    let mut offset = 0;

    while let Some(item_start) = xml[offset..].find("<item") {
        let abs_start = offset + item_start;
        let item_end = xml[abs_start..]
            .find("</item>")
            .map(|i| abs_start + i + 7)
            .unwrap_or(xml.len());

        let item_xml = &xml[abs_start..item_end];
        let title = extract_tag(item_xml, "title").unwrap_or_default();
        let link = extract_tag(item_xml, "link");
        let guid = extract_tag(item_xml, "guid");
        let description = extract_tag(item_xml, "description")
            .or_else(|| extract_cdata(item_xml, "content:encoded"))
            .unwrap_or_default();
        let pub_date = extract_tag(item_xml, "pubDate");

        let id = guid
            .or_else(|| link.clone())
            .unwrap_or_else(|| format!("rss-{}", entries.len()));

        entries.push(FeedEntry {
            updated_at_ms: pub_date.as_deref().and_then(rss_timestamp_ms),
            id,
            title,
            body: description,
            link,
            published: pub_date,
        });

        offset = item_end;
    }

    Ok(entries)
}

fn parse_atom(xml: &str) -> Result<Vec<FeedEntry>, String> {
    let mut entries = Vec::new();
    let mut offset = 0;

    while let Some(entry_start) = xml[offset..].find("<entry") {
        let abs_start = offset + entry_start;
        let entry_end = xml[abs_start..]
            .find("</entry>")
            .map(|i| abs_start + i + 8)
            .unwrap_or(xml.len());

        let entry_xml = &xml[abs_start..entry_end];
        let title = extract_tag(entry_xml, "title").unwrap_or_default();
        let id = extract_tag(entry_xml, "id").unwrap_or_else(|| format!("atom-{}", entries.len()));
        let content = extract_tag(entry_xml, "content")
            .or_else(|| extract_tag(entry_xml, "summary"))
            .unwrap_or_default();
        let link = extract_attr(entry_xml, "link", "href");
        let updated =
            extract_tag(entry_xml, "updated").or_else(|| extract_tag(entry_xml, "published"));

        entries.push(FeedEntry {
            updated_at_ms: updated.as_deref().and_then(rss_timestamp_ms),
            id,
            title,
            body: content,
            link,
            published: updated,
        });

        offset = entry_end;
    }

    Ok(entries)
}

/// Parse an RSS `pubDate` (RFC 2822) or Atom `updated`/`published` (RFC 3339)
/// timestamp into epoch milliseconds, so workspace sync can skip unchanged
/// entries instead of re-reading every item on every pass.
fn rss_timestamp_ms(value: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc2822(value)
        .or_else(|_| chrono::DateTime::parse_from_rfc3339(value))
        .map(|dt| dt.timestamp_millis())
        .ok()
}

/// Remove a surrounding `<![CDATA[ … ]]>` wrapper, if present.
fn unwrap_cdata(s: &str) -> &str {
    s.strip_prefix("<![CDATA[")
        .and_then(|inner| inner.strip_suffix("]]>"))
        .unwrap_or(s)
}

fn extract_tag(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let start = xml.find(&open)?;
    let content_start = xml[start..].find('>')? + start + 1;
    let end = xml[content_start..].find(&close)? + content_start;
    let content = &xml[content_start..end];
    let trimmed = content.trim();
    let unwrapped = unwrap_cdata(trimmed).trim();
    // CDATA content is literal text, so entity decoding applies only outside
    // a CDATA wrapper; decoding `&lt;` inside one would corrupt the content.
    if trimmed.starts_with("<![CDATA[") {
        Some(unwrapped.to_string())
    } else {
        Some(decode_xml_entities(unwrapped))
    }
}

fn extract_cdata(xml: &str, tag: &str) -> Option<String> {
    // `extract_tag` already unwraps `<![CDATA[ … ]]>`, so it serves both the
    // plain-text and CDATA-wrapped shapes.
    extract_tag(xml, tag)
}

fn extract_attr(xml: &str, tag: &str, attr: &str) -> Option<String> {
    let open = format!("<{tag} ");
    let start = xml.find(&open)?;
    let tag_end = xml[start..].find('>')? + start;
    let tag_str = &xml[start..tag_end];
    let attr_start = tag_str.find(&format!("{attr}=\""))? + attr.len() + 2;
    let attr_end = tag_str[attr_start..].find('"')? + attr_start;
    Some(tag_str[attr_start..attr_end].to_string())
}

fn decode_xml_entities(s: &str) -> String {
    // `&amp;` is decoded last so escaped entity text (`&amp;lt;` → `&lt;`)
    // survives as literal text instead of being decoded a second time.
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

#[cfg(test)]
#[path = "rss_tests.rs"]
mod tests;
