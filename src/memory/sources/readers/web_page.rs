//! Web page source reader.
//!
//! Fetches a single URL and extracts its text content. When a CSS
//! `selector` is configured, only matching elements are included;
//! otherwise the full page body is returned.
//!
//! The fetch-side SSRF guard (scheme/host policy plus a DNS resolver that
//! pins connections to globally routable addresses) lives in the shared
//! `ssrf` module, which the RSS reader uses too.

mod types;

use async_trait::async_trait;

use super::ssrf::{build_client, is_url_allowed, read_body_capped};
use types::SelectorSpec;

use crate::memory::config::MemoryConfig;
use crate::memory::error::MemoryEngineResult;
use crate::memory::sources::types::{
    ContentType, MemorySourceEntry, SourceContent, SourceItem, SourceKind,
};

use super::{into_engine_error, SourceReader};

pub struct WebPageReader;

#[async_trait]
impl SourceReader for WebPageReader {
    fn kind(&self) -> SourceKind {
        SourceKind::WebPage
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

impl WebPageReader {
    async fn list_items_inner(
        &self,
        source: &MemorySourceEntry,
        _config: &MemoryConfig,
    ) -> Result<Vec<SourceItem>, String> {
        let url = source
            .url
            .as_deref()
            .ok_or("web_page source requires a url")?;

        Ok(vec![SourceItem {
            id: url.to_string(),
            title: source.label.clone(),
            updated_at_ms: None,
        }])
    }

    async fn read_item_inner(
        &self,
        source: &MemorySourceEntry,
        item_id: &str,
        _config: &MemoryConfig,
    ) -> Result<SourceContent, String> {
        let url = if item_id.starts_with("http") {
            item_id.to_string()
        } else {
            source.url.clone().ok_or("web_page source requires a url")?
        };

        // SSRF guard: validate scheme and host, reject private/internal
        // targets, and refuse redirects that would escape that policy.
        let parsed = reqwest::Url::parse(&url).map_err(|e| format!("invalid URL: {e}"))?;
        if !is_url_allowed(&parsed) {
            return Err(format!(
                "web_page source requires an http(s) URL to a public host, got: {}",
                url.chars().take(64).collect::<String>()
            ));
        }

        tracing::debug!(
            host = %parsed.host_str().unwrap_or(""),
            selector = ?source.selector,
            "[memory_sources:web_page] reading item"
        );

        let client = build_client()?;
        let resp = client
            .get(parsed)
            .header("User-Agent", "openhuman")
            .send()
            .await
            .map_err(|e| format!("failed to fetch page: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!("page returned {}", resp.status()));
        }

        // Cap response body to 10 MiB so a hostile/giant page can't OOM us.
        // The read is streamed so the cap is enforced while downloading, not
        // after the whole body has been buffered into memory.
        const MAX_BODY_BYTES: u64 = 10 * 1024 * 1024;
        let bytes = read_body_capped(resp, MAX_BODY_BYTES).await?;
        let body = String::from_utf8_lossy(&bytes).into_owned();

        let extracted = if let Some(selector) = source.selector.as_deref() {
            extract_by_selector(&body, selector)
        } else {
            strip_html_tags(&body)
        };

        Ok(SourceContent {
            id: url.clone(),
            title: extract_title(&body).unwrap_or_else(|| url.clone()),
            body: extracted,
            content_type: ContentType::Plaintext,
            metadata: serde_json::json!({ "url": url }),
        })
    }
}

// ── Text extraction ─────────────────────────────────────────────────

fn extract_title(html: &str) -> Option<String> {
    let start = html.find("<title")?;
    let content_start = html[start..].find('>')? + start + 1;
    let end = html[content_start..].find("</title>")? + content_start;
    Some(html[content_start..end].trim().to_string())
}

fn parse_selector(selector: &str) -> Option<SelectorSpec> {
    let last = selector
        .trim()
        .rsplit(char::is_whitespace)
        .next()
        .unwrap_or("")
        .trim();
    if last.is_empty() {
        return None;
    }

    let mut spec = SelectorSpec {
        tag: None,
        id: None,
        classes: Vec::new(),
    };
    let mut part = String::new();
    let mut sep = ' '; // leading bare token is the tag
    for ch in last.chars() {
        match ch {
            '.' | '#' => {
                push_selector_part(&mut spec, &mut part, sep);
                sep = ch;
            }
            _ => part.push(ch),
        }
    }
    push_selector_part(&mut spec, &mut part, sep);

    if spec.tag.is_none() && spec.id.is_none() && spec.classes.is_empty() {
        None
    } else {
        Some(spec)
    }
}

fn push_selector_part(spec: &mut SelectorSpec, part: &mut String, sep: char) {
    let part = std::mem::take(part);
    if part.is_empty() {
        return;
    }
    match sep {
        '#' => spec.id = Some(part),
        '.' => spec.classes.push(part),
        _ => {
            if spec.tag.is_none() {
                spec.tag = Some(part);
            } else {
                spec.classes.push(part);
            }
        }
    }
}

/// Extract text from elements matching a simple CSS selector.
///
/// Falls back to the whole stripped page when the selector never matches
/// (rather than erroring), mirroring the reader's lenient posture for pages
/// whose structure changes between list and read time.
fn extract_by_selector(html: &str, selector: &str) -> String {
    let Some(spec) = parse_selector(selector) else {
        return strip_html_tags(html);
    };
    // Match against a script/style-stripped copy so JS strings and CSS rules
    // cannot be mistaken for nested elements, and so a selector that lands on
    // a `<script>` body finds nothing.
    let stripped = strip_script_and_style(html);
    let lower = stripped.to_ascii_lowercase();

    let mut result = String::new();
    let mut offset = 0;
    while let Some(rel) = find_next_element(&lower, &spec, offset) {
        let abs_start = offset + rel;
        let Some(gt_rel) = lower[abs_start..].find('>') else {
            // An unclosed opening tag (e.g. a truncated `<div` at the end of
            // the page) cannot contain a complete element, so stop scanning
            // rather than computing an out-of-bounds content range.
            break;
        };
        let gt = abs_start + gt_rel;
        let content_start = gt + 1;
        let tag = tag_name(&lower[abs_start..gt]).unwrap_or_default();

        let content_end = if tag.is_empty() {
            content_start
        } else {
            find_matching_close(&lower, &tag, content_start).unwrap_or(content_start)
        };
        let close_len = tag.len() + 3;

        if !result.is_empty() {
            result.push_str("\n\n");
        }
        result.push_str(&strip_html_tags(&stripped[content_start..content_end]));
        offset = content_end + close_len;
        if content_end == content_start {
            offset = content_start;
        }
    }

    if result.is_empty() {
        strip_html_tags(html)
    } else {
        result
    }
}

/// Find the offset (relative to `from`) of the next element opening tag that
/// matches `spec`, skipping comments, doctype/`!` declarations, and closing
/// tags.
fn find_next_element(lower_html: &str, spec: &SelectorSpec, from: usize) -> Option<usize> {
    let mut offset = from;
    while let Some(rel) = lower_html[offset..].find('<') {
        let abs = offset + rel;
        let after = &lower_html[abs + 1..];
        if after.starts_with('!') || after.starts_with('/') {
            offset = abs + 1;
            continue;
        }
        let tag_len = after
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
            .count();
        if tag_len == 0 {
            offset = abs + 1;
            continue;
        }
        let tag = &after[..tag_len];
        if let Some(expected) = &spec.tag {
            if !tag.eq_ignore_ascii_case(expected) {
                offset = abs + 1;
                continue;
            }
        }

        let gt = lower_html[abs..]
            .find('>')
            .map(|i| abs + i)
            .unwrap_or(lower_html.len());
        let open_tag = &lower_html[abs..gt];
        if let Some(expected_id) = &spec.id {
            if attr_value(open_tag, "id").as_deref() != Some(expected_id.as_str()) {
                offset = abs + 1;
                continue;
            }
        }
        if !spec.classes.is_empty() {
            let class_attr = attr_value(open_tag, "class").unwrap_or_default();
            let classes: std::collections::HashSet<&str> = class_attr.split_whitespace().collect();
            if spec.classes.iter().any(|c| !classes.contains(c.as_str())) {
                offset = abs + 1;
                continue;
            }
        }
        return Some(rel);
    }
    None
}

/// Read the tag name out of a lowercase opening tag (`<div class="x">` →
/// `div`).
fn tag_name(open_tag: &str) -> Option<&str> {
    let rest = open_tag.strip_prefix('<')?;
    Some(
        &rest[..rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
            .count()],
    )
}

/// Read an attribute value (lowercase name) from a lowercase opening tag,
/// handling single- and double-quoted values.
fn attr_value(open_tag: &str, name: &str) -> Option<String> {
    let mut rest = open_tag;
    while let Some(rel) = rest.find(name) {
        let after = &rest[rel + name.len()..];
        // Reject a prefix match inside a longer word (`classy=` is not
        // `class`).
        if rel > 0 {
            let prev = rest[..rel].chars().last().unwrap();
            if prev.is_ascii_alphanumeric() || prev == '-' || prev == '_' {
                rest = after;
                continue;
            }
        }
        let after = after.trim_start();
        if let Some(eq) = after.strip_prefix('=') {
            let eq = eq.trim_start();
            if let Some(v) = eq.strip_prefix('"') {
                if let Some(end) = v.find('"') {
                    return Some(v[..end].to_string());
                }
            } else if let Some(v) = eq.strip_prefix('\'') {
                if let Some(end) = v.find('\'') {
                    return Some(v[..end].to_string());
                }
            }
        }
        rest = after;
    }
    None
}

/// Find the offset of the `</tag>` that closes the element whose content
/// begins at `content_start`, accounting for nested same-named elements.
fn find_matching_close(lower_html: &str, tag: &str, content_start: usize) -> Option<usize> {
    let mut depth = 1usize;
    let mut offset = content_start;
    while offset < lower_html.len() {
        let rest = &lower_html[offset..];
        let next_open = rest.find(&format!("<{tag}")).and_then(|i| {
            let after = &rest[i + 1 + tag.len()..];
            let c = after.chars().next().unwrap_or('>');
            (c.is_whitespace() || c == '>' || c == '/').then_some(i)
        });
        let next_close = rest.find(&format!("</{tag}>"));
        let take_open = match (next_open, next_close) {
            (Some(no), Some(nc)) => no < nc,
            (Some(_), None) => true,
            (None, _) => false,
        };
        if take_open {
            depth += 1;
            offset += next_open.unwrap() + 1;
        } else {
            let nc = next_close?;
            depth -= 1;
            if depth == 0 {
                return Some(offset + nc);
            }
            offset += nc + 1;
        }
    }
    None
}

/// Remove `<script>` and `<style>` elements, including their contents, so
/// JavaScript source and CSS rules never reach the stripped text.
fn strip_script_and_style(html: &str) -> String {
    let mut current = html.to_string();
    let mut next = String::with_capacity(html.len());
    for tag in ["script", "style"] {
        next.clear();
        let mut scan = current.as_str();
        loop {
            let lower = scan.to_ascii_lowercase();
            let Some(open_rel) = lower.find(&format!("<{tag}")) else {
                next.push_str(scan);
                break;
            };
            let abs_open = open_rel;
            let Some(gt_rel) = scan[abs_open..].find('>') else {
                next.push_str(scan);
                break;
            };
            let content_start = abs_open + gt_rel + 1;
            let tail_lower = scan[content_start..].to_ascii_lowercase();
            let Some(close_rel) = tail_lower.find(&format!("</{tag}>")) else {
                // Unclosed element: the remainder of the page is (broken)
                // script/style content — drop it rather than leaking it into
                // the extracted text.
                next.push_str(&scan[..abs_open]);
                break;
            };
            let close_end = content_start + close_rel + tag.len() + 3;
            next.push_str(&scan[..abs_open]);
            scan = &scan[close_end..];
        }
        // This pass's output becomes the next pass's input.
        std::mem::swap(&mut current, &mut next);
    }
    current
}

fn strip_html_tags(html: &str) -> String {
    let html = strip_script_and_style(html);
    let mut result = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut last_was_space = false;

    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                if !last_was_space && !result.is_empty() {
                    result.push(' ');
                    last_was_space = true;
                }
            }
            _ if !in_tag => {
                if ch.is_whitespace() {
                    if !last_was_space {
                        result.push(' ');
                        last_was_space = true;
                    }
                } else {
                    result.push(ch);
                    last_was_space = false;
                }
            }
            _ => {}
        }
    }

    result.trim().to_string()
}

#[cfg(test)]
#[path = "web_page_tests.rs"]
mod tests;
