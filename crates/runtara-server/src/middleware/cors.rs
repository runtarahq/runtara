use std::time::Duration;

use axum::http::{HeaderName, HeaderValue, Method};
use tower_http::cors::{AllowHeaders, AllowOrigin, CorsLayer};

/// Build a CORS layer from the `CORS_ALLOWED_ORIGINS` environment variable.
///
/// | Value              | Behavior                                         |
/// |--------------------|--------------------------------------------------|
/// | `*`                | Any origin, no credentials                       |
/// | comma-separated    | Listed origins only, credentials enabled         |
/// | unset / empty      | Localhost defaults for dev, credentials enabled   |
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
    ];

    let expose_headers: Vec<HeaderName> = vec![
        HeaderName::from_static("content-length"),
        HeaderName::from_static("content-type"),
        HeaderName::from_static("x-request-id"),
    ];

    let base = CorsLayer::new()
        .allow_methods(allowed_methods)
        .allow_headers(AllowHeaders::list(allowed_headers))
        .expose_headers(expose_headers)
        .max_age(Duration::from_secs(86400));

    match std::env::var("CORS_ALLOWED_ORIGINS") {
        Ok(val) if val == "*" => {
            // Wildcard: any origin, no credentials (browsers reject credentials with *)
            base.allow_origin(AllowOrigin::any())
        }
        Ok(val) if !val.is_empty() => {
            // Specific origins from env var (comma-separated)
            let origins: Vec<HeaderValue> = val
                .split(',')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .filter_map(|s| HeaderValue::from_str(s).ok())
                .collect();
            base.allow_origin(AllowOrigin::list(origins))
                .allow_credentials(true)
        }
        _ => {
            // Default: localhost origins for development
            let origins = vec![
                HeaderValue::from_static("http://localhost:3000"),
                HeaderValue::from_static("http://localhost:8081"),
                HeaderValue::from_static("http://127.0.0.1:3000"),
                HeaderValue::from_static("http://127.0.0.1:8081"),
            ];
            base.allow_origin(AllowOrigin::list(origins))
                .allow_credentials(true)
        }
    }
}
