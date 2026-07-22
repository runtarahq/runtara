use std::time::Duration;

use axum::http::{HeaderName, HeaderValue, Method};
use tower_http::cors::{AllowHeaders, AllowOrigin, CorsLayer};

/// Fail-closed allowlist used when no origin is configured. The API echoes
/// sensitive headers (`Authorization`, `X-Org-Id`, `X-Runtara-*`) and enables
/// credentials, so a wildcard origin would let any website drive the API with
/// a stolen/pasted bearer token. We therefore fall back to local-development
/// front-ends only — never `*`. A deployment served on a public domain must
/// set `CORS_ALLOWED_ORIGINS`.
const DEFAULT_ALLOWED_ORIGINS: &[&str] = &[
    "http://localhost:3000",
    "http://localhost:8081",
    "http://127.0.0.1:3000",
    "http://127.0.0.1:8081",
];

/// Build a CORS layer from the `CORS_ALLOWED_ORIGINS` environment variable.
///
/// `CORS_ALLOWED_ORIGINS` is the single origin knob, **full-origin only**:
/// comma-separated `scheme://host[:port]` values; bare hosts, paths, and `*`
/// are rejected. (When a reverse proxy in front is the CORS authority,
/// runtara's own allowlist is moot for proxied traffic.)
///
/// | `CORS_ALLOWED_ORIGINS`      | Behavior                                   |
/// |-----------------------------|--------------------------------------------|
/// | full origin(s)              | Exactly those, credentials enabled         |
/// | `*`                         | Rejected — warns, default list             |
/// | unset / empty / all-invalid | [`DEFAULT_ALLOWED_ORIGINS`]                |
///
/// There is intentionally no "any origin" path: `*` is incompatible with a
/// credentialed API that accepts `Authorization`/`X-Org-Id`. Every path fails
/// closed to a non-empty allowlist, so the API is never wildcard-open.
pub fn build_cors_layer() -> CorsLayer {
    let allowed_methods = vec![
        Method::GET,
        Method::POST,
        Method::PUT,
        Method::PATCH,
        Method::DELETE,
        Method::OPTIONS,
        Method::HEAD,
    ];

    let allowed_headers: Vec<HeaderName> = vec![
        HeaderName::from_static("authorization"),
        HeaderName::from_static("content-type"),
        HeaderName::from_static("accept"),
        HeaderName::from_static("origin"),
        HeaderName::from_static("x-requested-with"),
        HeaderName::from_static("cache-control"),
        // Report workflow-action executes are idempotency-keyed; without this
        // the preflight silently strips the header path for cross-origin UIs.
        HeaderName::from_static("idempotency-key"),
    ];

    let expose_headers: Vec<HeaderName> = vec![
        HeaderName::from_static("content-length"),
        HeaderName::from_static("content-type"),
        HeaderName::from_static("x-request-id"),
    ];

    let origins = resolve_allowed_origins(std::env::var("CORS_ALLOWED_ORIGINS").ok().as_deref());

    CorsLayer::new()
        .allow_methods(allowed_methods)
        .allow_headers(AllowHeaders::list(allowed_headers))
        .expose_headers(expose_headers)
        .max_age(Duration::from_secs(86400))
        .allow_origin(AllowOrigin::list(origins))
        .allow_credentials(true)
}

/// True for a syntactically valid CORS origin: `scheme://host[:port]` with
/// `scheme` being http/https and no path, query, fragment, userinfo,
/// wildcard, or whitespace. (A bare host like `example.com` is rejected.)
fn is_valid_origin(s: &str) -> bool {
    let Some((scheme, authority)) = s.split_once("://") else {
        return false;
    };
    if !(scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")) {
        return false;
    }
    if authority.is_empty() {
        return false;
    }
    !authority
        .chars()
        .any(|c| matches!(c, '/' | '*' | '@' | '?' | '#') || c.is_whitespace())
}

/// Resolve `CORS_ALLOWED_ORIGINS` into a concrete origin allowlist. Always
/// returns a non-empty list — every invalid/empty/`*`/non-full-origin input
/// fails closed to [`DEFAULT_ALLOWED_ORIGINS`] so the API is never
/// wildcard-open.
fn resolve_allowed_origins(cors_allowed_origins: Option<&str>) -> Vec<HeaderValue> {
    let defaults = || {
        DEFAULT_ALLOWED_ORIGINS
            .iter()
            .filter_map(|s| HeaderValue::from_str(s).ok())
            .collect::<Vec<_>>()
    };

    let Some(val) = cors_allowed_origins else {
        return defaults();
    };
    let trimmed = val.trim();
    if trimmed.is_empty() {
        return defaults();
    }
    if trimmed == "*" {
        tracing::warn!("CORS_ALLOWED_ORIGINS='*' is not allowed for this credentialed API");
        return defaults();
    }

    let origins: Vec<HeaderValue> = trimmed
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .filter(|s| is_valid_origin(s))
        .filter_map(|s| HeaderValue::from_str(s).ok())
        .collect();

    if origins.is_empty() {
        tracing::warn!(
            "CORS_ALLOWED_ORIGINS contained no valid full origin \
             (expected scheme://host[:port]); falling back to the default \
             origin allowlist"
        );
        defaults()
    } else {
        origins
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strs(v: &[HeaderValue]) -> Vec<String> {
        v.iter().map(|h| h.to_str().unwrap().to_string()).collect()
    }

    // No env set → built-in defaults.
    fn def() -> Vec<String> {
        strs(&resolve_allowed_origins(None))
    }
    fn cors(v: &str) -> Vec<String> {
        strs(&resolve_allowed_origins(Some(v)))
    }

    #[test]
    fn unset_or_empty_uses_defaults() {
        assert!(def().contains(&"http://localhost:3000".to_string()));
        assert!(!def().is_empty());
        assert_eq!(cors(""), def());
        assert_eq!(cors("   "), def());
    }

    #[test]
    fn wildcard_is_rejected_and_falls_back() {
        let out = cors("*");
        assert_eq!(out, def());
        assert!(!out.contains(&"*".to_string()));
    }

    #[test]
    fn explicit_list_is_used_verbatim() {
        assert_eq!(
            cors("https://a.example.com, https://b.example.com"),
            vec![
                "https://a.example.com".to_string(),
                "https://b.example.com".to_string()
            ]
        );
        assert_eq!(
            cors("https://acme.example.com"),
            vec!["https://acme.example.com".to_string()]
        );
        assert_eq!(
            cors("https://a.example.com, http://b.example.com:8080"),
            vec![
                "https://a.example.com".to_string(),
                "http://b.example.com:8080".to_string()
            ]
        );
    }

    #[test]
    fn star_mixed_into_a_list_is_dropped_not_honored() {
        assert_eq!(
            cors("https://a.example.com, *"),
            vec!["https://a.example.com".to_string()]
        );
    }

    #[test]
    fn non_full_origins_are_rejected_and_fail_closed() {
        // Bare host, path, wildcard, bad scheme → invalid.
        assert_eq!(cors("acme.example.com"), def());
        assert_eq!(cors("https://acme.example.com/path"), def());
        assert_eq!(cors("https://acme.example.com/"), def());
        assert_eq!(cors("ftp://acme.example.com"), def());
        assert_eq!(cors(" , , "), def());
        // Mixed valid + invalid keeps only the valid full origin.
        assert_eq!(
            cors("acme.example.com, https://ok.example.com"),
            vec!["https://ok.example.com".to_string()]
        );
    }

    #[test]
    fn is_valid_origin_rules() {
        assert!(is_valid_origin("https://app.example.com"));
        assert!(is_valid_origin("http://localhost:3000"));
        assert!(is_valid_origin("https://a.b.c.example.com:8443"));
        assert!(!is_valid_origin("example.com"));
        assert!(!is_valid_origin("https://example.com/"));
        assert!(!is_valid_origin("https://example.com/path"));
        assert!(!is_valid_origin("https://*.example.com"));
        assert!(!is_valid_origin("https://user@example.com"));
        assert!(!is_valid_origin("ws://example.com"));
        assert!(!is_valid_origin("https://"));
        assert!(!is_valid_origin("*"));
    }
}
