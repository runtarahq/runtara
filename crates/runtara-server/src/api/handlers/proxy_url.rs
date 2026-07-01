// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Pure URL-pinning + egress-classification core for the internal proxy.
//!
//! This module holds the security-critical decision logic extracted out of
//! [`super::internal_proxy`] so it can be exhaustively unit-tested without
//! axum, a reqwest client, a database, or any environment reads.
//!
//! The proxy injects stored connection credentials server-side and forwards a
//! request to a URL the WASM agent supplies. Without a destination control, an
//! agent (or a prompt-injected AI agent that picks URLs) could redirect the
//! credentialed request to a host it controls and exfiltrate the secret. The
//! functions here are that control:
//!
//! * [`pin_url_to_base`] — fail-closed re-rooting of the agent URL onto the
//!   connection's base URL (host/scheme/port) **and** containment of the
//!   request path under the base path. Used for F1 (no/empty/unparseable base),
//!   F3 (base-path scope escape) and F4 (fail-open re-root).
//! * [`is_private_ip`] — SSRF address classifier, normalizing IPv4-mapped /
//!   IPv4-compatible IPv6 before the v6 ranges (F5).
//! * [`path_is_under`] — segment-wise path containment used by both the proxy
//!   and the presign path builder.
//!
//! Design rule: **everything here is pure** — no I/O, no env reads, no axum
//! types. Policy inputs (the http-base allowlist, whether path enforcement
//! applies) are passed in by the caller. The HTTP-status mapping for
//! [`ProxyReject`] lives in `internal_proxy.rs`, not here.

use std::net::{IpAddr, Ipv4Addr};

/// Why the proxy refused to forward a request. Pure; the HTTP-status mapping
/// lives in `internal_proxy.rs`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProxyReject {
    /// Connection-scoped request but the connection has no base URL at all (F1).
    NoBaseUrl,
    /// Connection-scoped request but the base URL is present-but-empty (F1).
    EmptyBaseUrl,
    /// Base URL does not parse, or has no host (F1/F4).
    UnparseableBaseUrl(String),
    /// Base URL scheme is not https and its host is not in the http allowlist (F1).
    NonHttpsBaseUrl(String),
    /// The agent-supplied URL does not parse (F4).
    UnparseableAgentUrl(String),
    /// The agent URL's normalized path escapes the connection's base path (F3).
    PathEscape {
        final_path: String,
        base_path: String,
    },
}

/// Per-request pinning policy. Pure — built by the caller from config.
#[derive(Debug, Clone)]
pub struct PinOptions {
    /// Enforce that the final path stays under the base path. Relaxed for
    /// object-store / MCP / signed integrations where the path is the payload.
    pub enforce_path_prefix: bool,
    /// Hosts for which an `http://` (non-TLS) base URL is acceptable. Empty
    /// means https-only (the fail-closed default). Host-scoped on purpose so a
    /// single dev/socat sidecar can be allowed without disabling TLS globally.
    pub allow_http_base_hosts: Vec<String>,
}

impl PinOptions {
    /// https-only, path enforced — the strict default.
    pub fn strict() -> Self {
        Self {
            enforce_path_prefix: true,
            allow_http_base_hosts: Vec::new(),
        }
    }
}

/// Re-root `agent_url` onto `base_url` and enforce path containment, fail-closed.
///
/// * `connection_scoped` — true when the request carries a connection id (i.e.
///   credentials will be / were injected). When true and there is no usable
///   base URL, the request is rejected so the credential is never forwarded to
///   an unpinned host. This is identity-based, not a "did we detect a credential
///   header" heuristic (which is bypassable).
///
/// On success returns the absolute URL string to forward (original
/// percent-encoding preserved). On any [`ProxyReject`] the caller must NOT send
/// the request.
pub fn pin_url_to_base(
    agent_url: &str,
    base_url: Option<&str>,
    connection_scoped: bool,
    opts: &PinOptions,
) -> Result<String, ProxyReject> {
    // ── F1: base URL must exist for a connection-scoped request ──────────────
    let base = match base_url.map(str::trim).filter(|s| !s.is_empty()) {
        Some(b) => b,
        None => {
            // No usable base. A connection-scoped request must never forward an
            // injected credential to an unpinned host, so reject. (A
            // non-connection request does not call this function.)
            return Err(match base_url {
                Some(_) => ProxyReject::EmptyBaseUrl, // present but blank/whitespace
                None => ProxyReject::NoBaseUrl,
            });
        }
    };
    let _ = connection_scoped; // documented above; pin is only invoked when scoped

    let base_parsed =
        url::Url::parse(base).map_err(|e| ProxyReject::UnparseableBaseUrl(e.to_string()))?;
    let base_host = match base_parsed.host_str() {
        Some(h) if !h.is_empty() => h.to_string(),
        _ => {
            return Err(ProxyReject::UnparseableBaseUrl(
                "base URL has no host".into(),
            ));
        }
    };

    // ── F1: scheme policy (https-only unless host is explicitly http-allowed) ─
    match base_parsed.scheme() {
        "https" => {}
        "http"
            if opts
                .allow_http_base_hosts
                .iter()
                .any(|h| h.eq_ignore_ascii_case(&base_host)) => {}
        _ => return Err(ProxyReject::NonHttpsBaseUrl(base.to_string())),
    }

    // ── Build the final URL re-rooted onto the base host/scheme/port ─────────
    let final_parsed = if agent_url.starts_with('/') {
        // Relative path: append under the base path, then let the url crate
        // resolve dot-segments. We join an absolute-path reference against the
        // base ORIGIN (path "/"), so the result is always on the base host —
        // a "//evil/x" or "/\evil/x" agent URL becomes a path under base, not a
        // host change. Query/fragment ride along in `agent_url`.
        let mut root = base_parsed.clone();
        root.set_path("/");
        root.set_query(None);
        root.set_fragment(None);
        let _ = root.set_username("");
        let _ = root.set_password(None);
        let combined = format!(
            "{}/{}",
            base_parsed.path().trim_end_matches('/'),
            agent_url.trim_start_matches('/')
        );
        root.join(&combined)
            .map_err(|e| ProxyReject::UnparseableAgentUrl(e.to_string()))?
    } else {
        // Absolute URL: graft the agent's path/query/fragment onto a clone of
        // the base URL. We never call set_host/set_scheme on the agent URL
        // (which can fail on odd schemes and was the F4 fail-open vector); the
        // host/scheme/port come entirely from the already-validated base, so a
        // re-root can never silently leave the attacker host in place.
        let agent = url::Url::parse(agent_url)
            .map_err(|e| ProxyReject::UnparseableAgentUrl(e.to_string()))?;
        let mut out = base_parsed.clone();
        out.set_path(agent.path());
        out.set_query(agent.query());
        out.set_fragment(agent.fragment());
        // Drop any userinfo carried by the base so it is never re-emitted, and
        // ignore the agent's userinfo entirely (host is base's).
        let _ = out.set_username("");
        let _ = out.set_password(None);
        out
    };

    // ── F3: path containment (decoded + dot-segment normalized) ──────────────
    if opts.enforce_path_prefix && !path_within_base(final_parsed.path(), base_parsed.path()) {
        return Err(ProxyReject::PathEscape {
            final_path: final_parsed.path().to_string(),
            base_path: base_parsed.path().to_string(),
        });
    }

    Ok(final_parsed.to_string())
}

/// True if `final_path` is contained under `base_path` after percent-decoding
/// (once, matching one upstream decode) and dot-segment normalization. Prefer
/// this over [`path_is_under`] when either path may carry percent-encoding or
/// `..` segments. Shared by the proxy pin and the presign path builder.
pub fn path_within_base(final_path: &str, base_path: &str) -> bool {
    let child = normalize_segments(&percent_decode(final_path));
    let base = normalize_segments(&percent_decode(base_path));
    path_is_under(&child, &base)
}

/// Segment-wise path containment: `child` is contained in `base` when it equals
/// `base` or sits below it as a path segment. NOT a raw string prefix, so
/// `/v2/tenantBxyz` is NOT under `/v2/tenantB`. A base of `/` (root) contains
/// everything. Inputs are expected to be already dot-segment normalized.
pub fn path_is_under(child: &str, base: &str) -> bool {
    let b = base.trim_end_matches('/');
    if b.is_empty() {
        return true; // base path is root → any path allowed
    }
    let c = child.trim_end_matches('/');
    c == b || c.starts_with(&format!("{b}/"))
}

/// Percent-decode a path ONCE (matching one upstream decode). `%2e`→`.`,
/// `%2f`→`/`, etc. Double-encoded sequences (`%252e`) decode to a literal
/// `%2e`, which a correctly-behaving upstream does not treat as a traversal —
/// so a single decode is the right amount for the containment check.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(h), Some(l)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2]))
        {
            out.push(h * 16 + l);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// RFC-3986 remove_dot_segments over a `/`-delimited path, also collapsing
/// empty segments (`//`). Returns an absolute path with a single leading `/`
/// and no trailing slash (except root `/`).
fn normalize_segments(path: &str) -> String {
    let mut stack: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => continue,
            ".." => {
                stack.pop();
            }
            s => stack.push(s),
        }
    }
    let mut out = String::from("/");
    out.push_str(&stack.join("/"));
    out
}

/// SSRF address classifier: is this IP one we must never let the proxy reach?
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

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(path_prefix: bool) -> PinOptions {
        PinOptions {
            enforce_path_prefix: path_prefix,
            allow_http_base_hosts: Vec::new(),
        }
    }

    // ── F1: base URL presence / validity ─────────────────────────────────────

    #[test]
    fn pin_rejects_absent_base_url_when_connection_scoped() {
        let r = pin_url_to_base("https://attacker.example/v1/x", None, true, &opts(true));
        assert_eq!(r, Err(ProxyReject::NoBaseUrl));
    }

    #[test]
    fn pin_rejects_empty_base_url() {
        for base in ["", "   "] {
            let r = pin_url_to_base("https://attacker.example/x", Some(base), true, &opts(true));
            assert_eq!(r, Err(ProxyReject::EmptyBaseUrl), "base={base:?}");
        }
    }

    #[test]
    fn pin_rejects_unparseable_base_url() {
        for base in ["://", "ht!tp://nope"] {
            let r = pin_url_to_base("https://x/y", Some(base), true, &opts(true));
            assert!(
                matches!(r, Err(ProxyReject::UnparseableBaseUrl(_))),
                "base={base:?} got {r:?}"
            );
        }
    }

    #[test]
    fn pin_rejects_base_without_host() {
        // mailto: parses but has no host.
        let r = pin_url_to_base("https://x/y", Some("mailto:a@b.com"), true, &opts(true));
        assert!(matches!(
            r,
            Err(ProxyReject::NonHttpsBaseUrl(_)) | Err(ProxyReject::UnparseableBaseUrl(_))
        ));
    }

    #[test]
    fn pin_rejects_non_https_base_by_default() {
        let r = pin_url_to_base("/x", Some("http://api.example.com"), true, &opts(true));
        assert!(matches!(r, Err(ProxyReject::NonHttpsBaseUrl(_))));
    }

    #[test]
    fn pin_allows_http_base_for_allowlisted_host() {
        let o = PinOptions {
            enforce_path_prefix: true,
            allow_http_base_hosts: vec!["127.0.0.1".into()],
        };
        let r = pin_url_to_base("/data", Some("http://127.0.0.1:9000/v1"), true, &o).unwrap();
        assert!(r.starts_with("http://127.0.0.1:9000/v1/data"), "got {r}");
        // a different http host is still rejected
        let r2 = pin_url_to_base("/data", Some("http://evil.example/v1"), true, &o);
        assert!(matches!(r2, Err(ProxyReject::NonHttpsBaseUrl(_))));
    }

    // ── Host pinning (F1) + F4 fail-closed ───────────────────────────────────

    #[test]
    fn pin_reroots_absolute_url_to_base_host() {
        let r = pin_url_to_base(
            "https://attacker.example/v2/tenantA/x",
            Some("https://api.example.com/v2/tenantA"),
            true,
            &opts(true),
        )
        .unwrap();
        let u = url::Url::parse(&r).unwrap();
        assert_eq!(u.host_str(), Some("api.example.com"));
        assert_eq!(u.path(), "/v2/tenantA/x");
        assert!(!r.contains("attacker.example"));
    }

    #[test]
    fn pin_forces_scheme_to_base() {
        // agent uses http, base is https → output must be https on base host.
        let r = pin_url_to_base(
            "http://attacker.example/api/x",
            Some("https://api.example.com/api"),
            true,
            &opts(true),
        )
        .unwrap();
        assert!(r.starts_with("https://api.example.com/api/x"), "got {r}");
    }

    #[test]
    fn pin_strips_userinfo_confusion() {
        let r = pin_url_to_base(
            "https://api.example.com@evil.example/api/x",
            Some("https://api.example.com/api"),
            true,
            &opts(true),
        )
        .unwrap();
        let u = url::Url::parse(&r).unwrap();
        assert_eq!(u.host_str(), Some("api.example.com"));
        assert_eq!(u.username(), "");
        assert!(!r.contains("evil.example"));
    }

    #[test]
    fn pin_preserves_query_and_fragment() {
        // Relative "/x" is appended under the base path (system convention).
        let r = pin_url_to_base(
            "/x?a=1&b=2#frag",
            Some("https://api.example.com/v2/tenantA"),
            true,
            &opts(true),
        )
        .unwrap();
        let u = url::Url::parse(&r).unwrap();
        assert_eq!(u.path(), "/v2/tenantA/x");
        assert_eq!(u.query(), Some("a=1&b=2"));
        assert_eq!(u.fragment(), Some("frag"));
    }

    // ── F3: base-path containment ────────────────────────────────────────────

    #[test]
    fn pin_rejects_absolute_same_host_path_escape() {
        let r = pin_url_to_base(
            "https://api.example.com/v2/tenantB/secret",
            Some("https://api.example.com/v2/tenantA"),
            true,
            &opts(true),
        );
        assert!(
            matches!(r, Err(ProxyReject::PathEscape { .. })),
            "got {r:?}"
        );
    }

    #[test]
    fn pin_rejects_relative_dotdot_escape() {
        let r = pin_url_to_base(
            "/../../v2/tenantB",
            Some("https://api.example.com/v2/tenantA"),
            true,
            &opts(true),
        );
        assert!(
            matches!(r, Err(ProxyReject::PathEscape { .. })),
            "got {r:?}"
        );
    }

    #[test]
    fn pin_rejects_encoded_dotdot_escape() {
        let base = Some("https://api.example.com/v2/tenantA");
        // Relative climbs above base via encoded dot-segments.
        for agent in ["/%2e%2e/%2e%2e/v2/tenantB", "/..%2f..%2fv2/tenantB"] {
            let r = pin_url_to_base(agent, base, true, &opts(true));
            assert!(
                matches!(r, Err(ProxyReject::PathEscape { .. })),
                "relative agent={agent} got {r:?}"
            );
        }
        // Absolute URL carrying encoded dot-segments in its path.
        let r = pin_url_to_base(
            "https://api.example.com/v2/tenantA/%2e%2e/%2e%2e/v2/tenantB",
            base,
            true,
            &opts(true),
        );
        assert!(
            matches!(r, Err(ProxyReject::PathEscape { .. })),
            "absolute encoded escape got {r:?}"
        );
    }

    #[test]
    fn pin_allows_relative_under_base() {
        // "/items" appends under base path "/v2/tenantA".
        let r = pin_url_to_base(
            "/items?page=2",
            Some("https://api.example.com/v2/tenantA"),
            true,
            &opts(true),
        )
        .unwrap();
        let u = url::Url::parse(&r).unwrap();
        assert_eq!(u.host_str(), Some("api.example.com"));
        assert_eq!(u.path(), "/v2/tenantA/items");
        assert_eq!(u.query(), Some("page=2"));
    }

    #[test]
    fn pin_allows_short_relative_appended_under_base() {
        // "/items" → "/v2/tenantA/items"
        let r = pin_url_to_base(
            "/items",
            Some("https://api.example.com/v2/tenantA"),
            true,
            &opts(true),
        )
        .unwrap();
        assert!(r.ends_with("/v2/tenantA/items"), "got {r}");
    }

    #[test]
    fn pin_double_slash_and_backslash_stay_on_base_host() {
        // These vectors try to hijack the HOST. The host must always stay base;
        // "evil.example" landing as a harmless PATH segment is fine.
        for agent in ["//evil.example/x", "/\\evil.example/x", "/..\\..\\x"] {
            let r = pin_url_to_base(
                agent,
                Some("https://api.example.com/base"),
                true,
                &opts(false), // isolate host check from path containment
            )
            .unwrap();
            let u = url::Url::parse(&r).unwrap();
            assert_eq!(
                u.host_str(),
                Some("api.example.com"),
                "agent={agent} got {r}"
            );
        }
    }

    #[test]
    fn pin_path_check_relaxed_allows_object_key_paths() {
        // S3-style: base host pinned, but path is the object key — no containment.
        let r = pin_url_to_base(
            "https://s3.amazonaws.com/bucket/a/b/key",
            Some("https://s3.amazonaws.com/bucket"),
            true,
            &opts(false),
        )
        .unwrap();
        let u = url::Url::parse(&r).unwrap();
        assert_eq!(u.host_str(), Some("s3.amazonaws.com"));
        assert_eq!(u.path(), "/bucket/a/b/key");
    }

    #[test]
    fn pin_allows_path_equality_for_single_endpoint() {
        // MCP-shape: base IS the full endpoint; equal path is contained.
        let r = pin_url_to_base(
            "https://mcp.example.com/jsonrpc",
            Some("https://mcp.example.com/jsonrpc"),
            true,
            &opts(true),
        )
        .unwrap();
        assert!(r.starts_with("https://mcp.example.com/jsonrpc"), "got {r}");
    }

    #[test]
    fn pin_base_origin_only_allows_any_path() {
        // base path "/" (bare origin like hubspot/stripe) → any path allowed.
        let r = pin_url_to_base(
            "/crm/v3/objects/contacts",
            Some("https://api.hubapi.com"),
            true,
            &opts(true),
        )
        .unwrap();
        assert!(r.ends_with("/crm/v3/objects/contacts"), "got {r}");
    }

    #[test]
    fn pin_unparseable_agent_url_rejected() {
        let r = pin_url_to_base(
            "not a url",
            Some("https://api.example.com"),
            true,
            &opts(true),
        );
        assert!(
            matches!(r, Err(ProxyReject::UnparseableAgentUrl(_))),
            "got {r:?}"
        );
    }

    // ── path_is_under is segment-wise, not raw prefix ────────────────────────

    #[test]
    fn path_is_under_is_segmentwise() {
        assert!(!path_is_under("/v2/tenantBxyz", "/v2/tenantB"));
        assert!(path_is_under("/v2/tenantB/x", "/v2/tenantB"));
        assert!(path_is_under("/v2/tenantB", "/v2/tenantB"));
        assert!(path_is_under("/anything", "/"));
        assert!(path_is_under("/anything", ""));
    }

    #[test]
    fn deep_multisegment_base_path_allows_child_blocks_sibling() {
        let base = "https://api.businesscentral.dynamics.com/v2.0/production/api/v2.0";
        // Relative child appends under the deep base path.
        let ok = pin_url_to_base("/companies", Some(base), true, &opts(true));
        assert!(ok.is_ok(), "child under deep base should pass: {ok:?}");
        // Absolute URL to a sibling API version on the same host is blocked.
        let escape = pin_url_to_base(
            "https://api.businesscentral.dynamics.com/v2.0/production/api/beta/companies",
            Some(base),
            true,
            &opts(true),
        );
        assert!(
            matches!(escape, Err(ProxyReject::PathEscape { .. })),
            "sibling version must be blocked: {escape:?}"
        );
    }

    // ── F5: is_private_ip ────────────────────────────────────────────────────

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn is_private_ip_blocks_ipv4_ranges() {
        for s in [
            "127.0.0.1",
            "10.0.0.1",
            "172.16.0.1",
            "192.168.0.1",
            "169.254.169.254", // cloud metadata
            "100.64.0.1",      // CGNAT
            "100.127.255.255", // CGNAT upper
            "0.0.0.0",
            "255.255.255.255",
        ] {
            assert!(is_private_ip(&ip(s)), "{s} should be private");
        }
    }

    #[test]
    fn is_private_ip_allows_public_ipv4() {
        for s in ["8.8.8.8", "1.1.1.1", "100.63.255.255", "100.128.0.0"] {
            assert!(!is_private_ip(&ip(s)), "{s} should be public");
        }
    }

    #[test]
    fn is_private_ip_blocks_ipv4_mapped_ipv6() {
        for s in [
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "::ffff:169.254.169.254",
        ] {
            assert!(is_private_ip(&ip(s)), "{s} (mapped) should be private");
        }
        assert!(
            !is_private_ip(&ip("::ffff:8.8.8.8")),
            "mapped public should be public"
        );
    }

    #[test]
    fn is_private_ip_blocks_ipv4_compatible_ipv6() {
        assert!(is_private_ip(&ip("::127.0.0.1")), "compatible loopback");
        assert!(
            is_private_ip(&ip("::169.254.169.254")),
            "compatible metadata"
        );
    }

    #[test]
    fn is_private_ip_blocks_ipv6_ranges() {
        for s in ["::1", "::", "fc00::1", "fd12::1", "fe80::1"] {
            assert!(is_private_ip(&ip(s)), "{s} should be private");
        }
        assert!(!is_private_ip(&ip("2606:4700::1111")), "public v6");
    }
}
