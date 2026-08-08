use super::*;

// ── SSRF guard ──────────────────────────────────────────────────────

#[test]
fn is_url_allowed_accepts_public_http_urls() {
    assert!(is_url_allowed(
        &reqwest::Url::parse("https://example.com").unwrap()
    ));
    assert!(is_url_allowed(
        &reqwest::Url::parse("http://example.com/x").unwrap()
    ));
    assert!(is_url_allowed(
        &reqwest::Url::parse("https://sub.example.com").unwrap()
    ));
    assert!(is_url_allowed(
        &reqwest::Url::parse("https://8.8.8.8").unwrap()
    ));
}

#[test]
fn is_url_allowed_rejects_private_and_internal_targets() {
    // Private / loopback / link-local IP literals.
    assert!(!is_url_allowed(
        &reqwest::Url::parse("http://127.0.0.1").unwrap()
    ));
    assert!(!is_url_allowed(
        &reqwest::Url::parse("http://10.0.0.1").unwrap()
    ));
    assert!(!is_url_allowed(
        &reqwest::Url::parse("http://192.168.1.1").unwrap()
    ));
    assert!(!is_url_allowed(
        &reqwest::Url::parse("http://169.254.169.254").unwrap()
    ));
    assert!(!is_url_allowed(
        &reqwest::Url::parse("http://[::1]").unwrap()
    ));
    // Internal service names and local-only names.
    assert!(!is_url_allowed(
        &reqwest::Url::parse("http://localhost").unwrap()
    ));
    assert!(!is_url_allowed(
        &reqwest::Url::parse("http://mongo").unwrap()
    ));
    assert!(!is_url_allowed(
        &reqwest::Url::parse("http://service.internal").unwrap()
    ));
    // Non-http scheme.
    assert!(!is_url_allowed(
        &reqwest::Url::parse("ftp://example.com").unwrap()
    ));
    assert!(!is_url_allowed(
        &reqwest::Url::parse("file:///etc/passwd").unwrap()
    ));
}

#[test]
fn is_blocked_host_rejects_ip_ranges_and_local_names() {
    let blocked = [
        "127.0.0.1",
        "0.0.0.0",
        "10.0.0.1",
        "172.16.0.1",
        "192.168.0.1",
        "169.254.169.254",
        "100.64.0.1", // CGNAT
        "192.0.0.1",  // IETF protocol assignments
        "localhost",
        "foo.local",
        "bar.internal",
        "mongo",
        "::1",
        "fc00::1",                // unique-local
        "fe80::1",                // link-local
        "::ffff:127.0.0.1",       // IPv4-mapped loopback literal
        "::ffff:10.0.0.1",        // IPv4-mapped private literal
        "::ffff:169.254.169.254", // IPv4-mapped link-local / cloud metadata
        // Special IPv4 literals that are not globally routable: multicast,
        // broadcast, documentation, benchmarking, and reserved ranges. A
        // literal never goes through DNS resolution, so the text check is the
        // only line of defense for these.
        "224.0.0.1",       // multicast
        "255.255.255.255", // broadcast
        "192.0.2.1",       // documentation
        "198.51.100.1",    // documentation
        "203.0.113.1",     // documentation
        "198.18.0.1",      // benchmarking
        "240.0.0.1",       // reserved
    ];
    for host in blocked {
        assert!(is_blocked_host(host), "expected {host:?} to be blocked");
    }
}

#[test]
fn is_blocked_host_accepts_public_hosts() {
    let allowed = [
        "8.8.8.8",
        "1.1.1.1",
        "example.com",
        "sub.example.com",
        "example.co.uk",
        "8.8.8.8.",    // trailing dot is normalized away
        "EXAMPLE.com", // case-insensitive
        "2001:4860:4860::8888",
        "::ffff:8.8.8.8", // IPv4-mapped public literal
    ];
    for host in allowed {
        assert!(!is_blocked_host(host), "expected {host:?} to be allowed");
    }
}

// ── resolved-address (DNS) SSRF classification ──────────────────────

fn public_ip(s: &str) -> IpAddr {
    s.parse().expect("valid ip literal")
}

#[test]
fn is_public_ip_rejects_internal_and_special_ranges() {
    let blocked = [
        "127.0.0.1",              // loopback
        "0.0.0.0",                // unspecified
        "10.0.0.1",               // private
        "172.16.0.1",             // private
        "192.168.1.1",            // private
        "169.254.169.254",        // link-local / cloud metadata
        "100.64.0.1",             // CGNAT
        "192.0.0.1",              // IETF protocol assignments
        "224.0.0.1",              // multicast
        "255.255.255.255",        // broadcast
        "192.0.2.1",              // documentation
        "198.51.100.1",           // documentation
        "203.0.113.1",            // documentation
        "198.18.0.1",             // benchmarking
        "240.0.0.1",              // reserved
        "::1",                    // loopback
        "::",                     // unspecified
        "fc00::1",                // unique-local
        "fe80::1",                // link-local
        "ff00::1",                // multicast
        "2001:db8::1",            // documentation
        "::ffff:127.0.0.1",       // IPv4-mapped loopback
        "::ffff:169.254.169.254", // IPv4-mapped link-local
    ];
    for s in blocked {
        assert!(!is_public_ip(public_ip(s)), "expected {s:?} to be rejected");
    }
}

#[test]
fn is_public_ip_accepts_global_addresses() {
    let allowed = [
        "8.8.8.8",
        "1.1.1.1",
        "93.184.216.34",
        "2001:4860:4860::8888",
        "2606:4700:4700::1111",
        "::ffff:8.8.8.8", // IPv4-mapped public
    ];
    for s in allowed {
        assert!(is_public_ip(public_ip(s)), "expected {s:?} to be allowed");
    }
}
