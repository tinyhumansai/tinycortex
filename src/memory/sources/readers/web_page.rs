//! Web page source reader.
//!
//! Fetches a single URL and extracts its text content. When a CSS
//! `selector` is configured, only matching elements are included;
//! otherwise the full page body is returned.
//!
//! ## SSRF guard
//!
//! `read_item_inner` only fetches `http(s)`
//! URLs and refuses hosts that could target non-public resources: loopback /
//! private / link-local / unique-local IP literals, `localhost`, `.local` /
//! `.internal` names, and single-label hostnames (internal service names).
//! Redirects are re-checked against the same policy, so a public URL cannot
//! redirect the fetch onto an internal host.

use async_trait::async_trait;

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
        const MAX_BODY_BYTES: u64 = 10 * 1024 * 1024;
        if let Some(len) = resp.content_length() {
            if len > MAX_BODY_BYTES {
                return Err(format!(
                    "page body exceeds {MAX_BODY_BYTES}-byte limit (Content-Length={len})"
                ));
            }
        }

        let bytes = resp
            .bytes()
            .await
            .map_err(|e| format!("failed to read page body: {e}"))?;
        if bytes.len() as u64 > MAX_BODY_BYTES {
            return Err(format!(
                "page body exceeds {MAX_BODY_BYTES}-byte limit (read {} bytes)",
                bytes.len()
            ));
        }
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

// ── HTTP client + SSRF policy ───────────────────────────────────────

/// Build the HTTP client with a redirect policy that re-applies the SSRF
/// host/scheme check to every redirect hop.
fn build_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if is_url_allowed(attempt.url()) {
                attempt.follow()
            } else {
                // `stop` returns the redirect response to the caller instead
                // of following it; the read then fails on the non-2xx status.
                attempt.stop()
            }
        }))
        .build()
        .map_err(|e| format!("failed to build http client: {e}"))
}

/// Whether a URL may be fetched: `http(s)` scheme against a public host.
fn is_url_allowed(url: &reqwest::Url) -> bool {
    match url.scheme() {
        "http" | "https" => {}
        _ => return false,
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    !is_blocked_host(host)
}

/// Reject hosts that could target non-public resources: IP literals in
/// loopback / private / link-local / unique-local / unspecified ranges, plus
/// `localhost`, `.local` / `.internal` names, and single-label hostnames
/// (internal service names such as `mongo` or `redis`).
fn is_blocked_host(host: &str) -> bool {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() {
        return true;
    }
    if let Ok(ip) = host.parse::<std::net::Ipv4Addr>() {
        return is_private_ipv4(ip);
    }
    if let Ok(ip) = host.parse::<std::net::Ipv6Addr>() {
        return is_private_ipv6(ip);
    }
    if host == "localhost" || host.ends_with(".local") || host.ends_with(".internal") {
        return true;
    }
    // A single-label name is an internal-service name, not a public domain.
    !host.contains('.')
}

fn is_private_ipv4(ip: std::net::Ipv4Addr) -> bool {
    if ip.is_loopback() || ip.is_private() || ip.is_link_local() || ip.is_unspecified() {
        return true;
    }
    let o = ip.octets();
    // 100.64.0.0/10 CGNAT and 192.0.0.0/24 (IETF protocol assignments).
    (o[0] == 100 && o[1] & 0xc0 == 0x40) || (o[0] == 192 && o[1] == 0)
}

fn is_private_ipv6(ip: std::net::Ipv6Addr) -> bool {
    if ip.is_loopback() || ip.is_unspecified() {
        return true;
    }
    let o = ip.octets();
    // Unique-local fc00::/7 and link-local fe80::/10.
    (o[0] == 0xfc || o[0] == 0xfd) || (o[0] == 0xfe && o[1] & 0xc0 == 0x80)
}

// ── Text extraction ─────────────────────────────────────────────────

fn extract_title(html: &str) -> Option<String> {
    let start = html.find("<title")?;
    let content_start = html[start..].find('>')? + start + 1;
    let end = html[content_start..].find("</title>")? + content_start;
    Some(html[content_start..end].trim().to_string())
}

/// A parsed simple CSS selector: optional tag name, optional id, and class
/// names. Supports `tag`, `tag.class`, `tag#id`, `.class`, `#id`, and stacked
/// classes (`tag.a.b`, `.a.b`). A descendant/child chain (`div.content p`)
/// targets the final compound selector — a full CSS engine is out of scope for
/// this reader.
struct SelectorSpec {
    tag: Option<String>,
    id: Option<String>,
    classes: Vec<String>,
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
        let gt = lower[abs_start..]
            .find('>')
            .map(|i| abs_start + i)
            .unwrap_or(lower.len());
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
