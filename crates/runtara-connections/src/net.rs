// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Shared network egress guards for credential-bearing HTTP.
//!
//! Home of the SSRF address classifier, the egress host allowlist, and the
//! hardened reqwest client (no redirects + DNS-guarded resolver). Lives in
//! `runtara-connections` because both this crate (OAuth token/refresh/revoke
//! POSTs) and `runtara-server` (internal proxy egress) need them, and the
//! server already depends on this crate. The server re-exports these so its
//! existing call sites keep working.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

/// SSRF address classifier: is this IP one we must never let credentialed
/// egress reach?
///
/// Covers loopback, RFC-1918 private, link-local (incl. cloud metadata
/// 169.254.169.254), CGNAT 100.64/10, broadcast, unspecified, IPv6 ULA
/// (fc00::/7) and link-local (fe80::/10). Crucially, IPv4-mapped
/// (`::ffff:a.b.c.d`) and IPv4-compatible (`::a.b.c.d`) IPv6 addresses are
/// decoded to their embedded v4 first, so `::ffff:127.0.0.1` is classified as
/// loopback (the F5 bypass).
pub fn is_private_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_unspecified()
                // CGNAT 100.64.0.0/10
                || (v4.octets()[0] == 100 && (64..=127).contains(&v4.octets()[1]))
        }
        IpAddr::V6(v6) => {
            // IPv4-mapped ::ffff:a.b.c.d → classify the embedded v4.
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_private_ip(&IpAddr::V4(v4));
            }
            if v6.is_loopback() || v6.is_unspecified() {
                return true;
            }
            // IPv4-compatible ::a.b.c.d (high 96 bits zero) → classify embedded v4.
            if v6.segments()[..6].iter().all(|s| *s == 0) {
                let s = v6.segments();
                let v4 = Ipv4Addr::new(
                    (s[6] >> 8) as u8,
                    (s[6] & 0xff) as u8,
                    (s[7] >> 8) as u8,
                    (s[7] & 0xff) as u8,
                );
                if !v4.is_unspecified() {
                    return is_private_ip(&IpAddr::V4(v4));
                }
            }
            // ULA fc00::/7 and link-local fe80::/10.
            (v6.segments()[0] & 0xfe00) == 0xfc00 || (v6.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}

/// Dev/test escape hatch for private-host egress (`RUNTARA_PROXY_ALLOWED_HOSTS`,
/// comma-separated, host or host:port). Empty = fail closed. Read once.
pub fn allowed_private_hosts() -> &'static [String] {
    static HOSTS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    HOSTS
        .get_or_init(|| {
            std::env::var("RUNTARA_PROXY_ALLOWED_HOSTS")
                .unwrap_or_default()
                .split(',')
                .map(|s| s.trim().to_ascii_lowercase())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .as_slice()
}

/// Host-only variant of the SSRF allowlist check, used by the egress client's
/// DNS resolver (which sees a bare host, no port). Matches a bare-host entry or
/// any host:port entry for that host.
pub fn host_is_allowlisted_for_egress(host: &str) -> bool {
    let allowed = allowed_private_hosts();
    if allowed.is_empty() {
        return false;
    }
    let h = host.to_ascii_lowercase();
    allowed
        .iter()
        .any(|entry| entry == &h || entry.starts_with(&format!("{h}:")))
}

/// reqwest DNS resolver that refuses to hand back addresses for a host that
/// resolves (even partially) to a private/internal range. Because reqwest
/// connects only to addresses the resolver returns, the vetted address is the
/// dialed address — closing the resolve-then-reconnect (DNS rebinding) TOCTOU.
pub struct GuardedResolver;

impl reqwest::dns::Resolve for GuardedResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        Box::pin(async move {
            let host = name.as_str().to_string();
            // Dev/test escape hatch — e.g. a loopback emulator or wiremock —
            // bypasses the private-IP check.
            let allowlisted = host_is_allowlisted_for_egress(&host);

            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })?
                .collect();

            if !allowlisted && addrs.iter().any(|a| is_private_ip(&a.ip())) {
                return Err(format!(
                    "egress blocked: host '{host}' resolves to a private/internal address"
                )
                .into());
            }

            let iter: reqwest::dns::Addrs = Box::new(addrs.into_iter());
            Ok(iter)
        })
    }
}

/// Build a hardened client for credential-bearing egress (OAuth token /
/// refresh / revoke POSTs, proxy upstream calls):
///
/// * redirects are NOT followed — a 3xx from a user-supplied token endpoint
///   must not carry the client secret / Basic header to another host;
/// * DNS is guarded — a host resolving to any private/internal address is
///   rejected outright (see [`GuardedResolver`]).
///
/// Construct once and reuse — it pools connections like any `reqwest::Client`.
pub fn build_hardened_client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .dns_resolver(Arc::new(GuardedResolver))
        .build()
        .expect("hardened egress client should build")
}

/// Save-time validation for a user-supplied credentialed endpoint URL:
/// https-only (unless the host is on the `RUNTARA_CONNECTION_ALLOW_HTTP_HOSTS`
/// dev allowlist the caller resolves), non-empty host, and — when the host is
/// an IP *literal* — not a private/internal address.
///
/// Deliberately does NOT resolve DNS (save runs in a request handler; a
/// resolve-here check would be TOCTOU anyway). The authoritative guard is the
/// connect-time [`GuardedResolver`] in [`build_hardened_client`].
pub fn validate_public_url(
    raw: &str,
    http_allowed_for_host: impl Fn(&str) -> bool,
) -> Result<(), String> {
    let parsed = url::Url::parse(raw).map_err(|e| format!("invalid URL: {e}"))?;
    let host = parsed.host_str().unwrap_or("");
    if host.is_empty() {
        return Err("URL must include a host".to_string());
    }
    match parsed.scheme() {
        "https" => {}
        "http" if http_allowed_for_host(host) => {}
        "http" => {
            return Err(format!(
                "URL must use https (http is allowed only for hosts in RUNTARA_CONNECTION_ALLOW_HTTP_HOSTS): {host}"
            ));
        }
        other => return Err(format!("unsupported URL scheme '{other}'")),
    }
    // Literal-IP hosts are classifiable without DNS; private literals are
    // rejected here, everything else is enforced at connect time.
    if let Ok(ip) = host.trim_matches(['[', ']']).parse::<IpAddr>()
        && is_private_ip(&ip)
        && !http_allowed_for_host(host)
    {
        return Err(format!("URL host {host} is a private/internal address"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn is_private_ip_blocks_ipv4_ranges() {
        for s in [
            "127.0.0.1",
            "10.0.0.1",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "255.255.255.255",
        ] {
            assert!(is_private_ip(&ip(s)), "{s} should be private");
        }
    }

    #[test]
    fn is_private_ip_allows_public_ipv4() {
        for s in ["8.8.8.8", "1.1.1.1", "93.184.216.34", "100.128.0.1"] {
            assert!(!is_private_ip(&ip(s)), "{s} should be public");
        }
    }

    #[test]
    fn is_private_ip_blocks_ipv4_mapped_and_compatible_ipv6() {
        for s in [
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "::ffff:169.254.169.254",
        ] {
            assert!(is_private_ip(&ip(s)), "{s} (mapped) should be private");
        }
        assert!(is_private_ip(&ip("::127.0.0.1")), "compatible loopback");
    }

    #[test]
    fn is_private_ip_blocks_ipv6_ranges() {
        for s in ["::1", "fc00::1", "fd12:3456::1", "fe80::1"] {
            assert!(is_private_ip(&ip(s)), "{s} should be private");
        }
        assert!(!is_private_ip(&ip("2001:4860:4860::8888")), "public v6");
    }

    #[test]
    fn validate_public_url_enforces_https_host_and_literal_ips() {
        let no_http = |_: &str| false;
        // Happy path.
        assert!(validate_public_url("https://api.example.com/token", no_http).is_ok());
        // http rejected unless allowlisted.
        assert!(validate_public_url("http://api.example.com/token", no_http).is_err());
        assert!(validate_public_url("http://127.0.0.1:9999/token", |h| h == "127.0.0.1").is_ok());
        // Private literal IPs rejected even over https.
        assert!(validate_public_url("https://169.254.169.254/latest", no_http).is_err());
        assert!(validate_public_url("https://10.0.0.5/token", no_http).is_err());
        assert!(validate_public_url("https://[::1]/token", no_http).is_err());
        // Public literal is fine.
        assert!(validate_public_url("https://93.184.216.34/token", no_http).is_ok());
        // Junk.
        assert!(validate_public_url("not a url", no_http).is_err());
        assert!(validate_public_url("ftp://example.com", no_http).is_err());
    }
}
