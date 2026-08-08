//! Shared SSRF guard and fetch hygiene for the network source readers.
//!
//! The web-page and RSS readers both fetch user-configured URLs, so they share
//! the policy in this module.
//!
//! The hostname *text* check (`is_blocked_host`) rejects private IP literals,
//! `localhost`, `.local` / `.internal` names, and single-label hostnames, but
//! a public-looking name can resolve to a loopback / private / link-local
//! address (including the cloud-metadata `169.254.169.254`) at lookup time.
//! `PublicOnlyResolver` therefore vets the resolved addresses and only lets the
//! connection proceed to a globally routable IP, so the request is pinned to an
//! address we have already allowed (no re-resolution between the check and the
//! connect). Redirects are re-checked through `is_url_allowed` so a public URL
//! cannot bounce the fetch onto an internal host.
//!
//! `read_body_capped` streams a response body and stops at a byte cap, so a
//! hostile or gigantic page/feed cannot OOM the process before the size check
//! runs.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;

use futures::stream::StreamExt;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};

/// Build an HTTP client with a redirect policy that re-applies the SSRF
/// host/scheme check to every redirect hop, and a DNS resolver that only
/// yields globally routable addresses.
pub(super) fn build_client() -> Result<reqwest::Client, String> {
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
        .dns_resolver(Arc::new(PublicOnlyResolver))
        .build()
        .map_err(|e| format!("failed to build http client: {e}"))
}

/// Stream a response body, failing once it exceeds `max` bytes.
///
/// `Response::bytes()` buffers the entire body before any size check, so a
/// server that omits or understates `Content-Length` (for example a chunked
/// response) could OOM the process despite the cap. Reading incrementally
/// enforces the limit while the bytes arrive.
pub(super) async fn read_body_capped(resp: reqwest::Response, max: u64) -> Result<Vec<u8>, String> {
    // Trust a truthful Content-Length up front so a known-huge body is
    // rejected before the first byte is read.
    if let Some(len) = resp.content_length() {
        if len > max {
            return Err(format!(
                "response body exceeds {max}-byte limit (Content-Length={len})"
            ));
        }
    }

    let mut body = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("failed to read response body: {e}"))?;
        body.extend_from_slice(&chunk);
        if body.len() as u64 > max {
            return Err(format!(
                "response body exceeds {max}-byte limit (read {} bytes)",
                body.len()
            ));
        }
    }
    Ok(body)
}

/// A DNS resolver that only yields globally routable addresses.
///
/// The text-based `is_blocked_host` check rejects private IP *literals* and
/// local hostnames, but a public-looking hostname can resolve to a loopback,
/// private, link-local, or cloud-metadata address (`169.254.169.254`) at
/// lookup time. Installing this resolver means reqwest connects to addresses
/// we have already vetted: a hostname whose current resolution is non-public
/// fails the request instead of silently reaching an internal service, and the
/// validated address is the one the connection is pinned to (no re-resolution
/// between the check and the connect).
#[derive(Debug, Default)]
struct PublicOnlyResolver;

impl Resolve for PublicOnlyResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map_err(|e| box_err(e))?
                .filter(|addr| is_public_ip(addr.ip()))
                .collect();
            if addrs.is_empty() {
                return Err(box_err(std::io::Error::new(
                    std::io::ErrorKind::AddrNotAvailable,
                    format!("host {host} resolved to no public addresses"),
                )));
            }
            Ok(Box::new(addrs.into_iter()) as Addrs)
        })
    }
}

fn box_err(
    e: impl std::error::Error + Send + Sync + 'static,
) -> Box<dyn std::error::Error + Send + Sync> {
    Box::new(e)
}

/// Whether `ip` is a globally routable address — the resolved-address half of
/// the SSRF guard. Mirrors the literal/name policy in `is_blocked_host`:
/// loopback, private, link-local, unique-local, multicast, broadcast,
/// unspecified, and documentation/reserved ranges are not fetchable.
fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_ipv4(v4),
        IpAddr::V6(v6) => is_public_ipv6(v6),
    }
}

fn is_public_ipv4(ip: Ipv4Addr) -> bool {
    if is_private_ipv4(ip) || ip.is_multicast() || ip.is_broadcast() {
        return false;
    }
    let o = ip.octets();
    // Documentation (192.0.2.0/24, 198.51.100.0/24, 203.0.113.0/24),
    // benchmarking (198.18.0.0/15), and reserved (240.0.0.0/4) ranges are not
    // globally routable.
    !((o[0] == 192 && o[1] == 0 && o[2] == 2)
        || (o[0] == 198 && o[1] == 51 && o[2] == 100)
        || (o[0] == 203 && o[1] == 0 && o[2] == 113)
        || (o[0] == 198 && o[1] == 18)
        || o[0] >= 240)
}

fn is_public_ipv6(ip: Ipv6Addr) -> bool {
    if is_private_ipv6(ip) || ip.is_multicast() {
        return false;
    }
    let o = ip.octets();
    // Documentation prefix 2001:db8::/32.
    if o[0] == 0x20 && o[1] == 0x01 && o[2] == 0x0d && o[3] == 0xb8 {
        return false;
    }
    // IPv4-mapped (`::ffff:a.b.c.d`) delegate to the embedded IPv4, so a
    // mapped loopback/private address stays blocked.
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_public_ipv4(v4);
    }
    true
}

/// Whether a URL may be fetched: `http(s)` scheme against a public host.
pub(super) fn is_url_allowed(url: &reqwest::Url) -> bool {
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

#[cfg(test)]
#[path = "ssrf_tests.rs"]
mod tests;
