use axum::{
    body::Body,
    extract::Request,
    http::{
        HeaderValue,
        header::{CONTENT_SECURITY_POLICY, HeaderName},
    },
    middleware::Next,
    response::Response,
};

/// Middleware that adds security headers to every response on the public server.
///
/// Matches the headers previously set by the API gateway's `header_filter_by_lua_block`.
/// HSTS is deliberately omitted — it stays at the TLS termination layer (gateway/LB).
///
/// CSP is applied only when a handler hasn't already set one. That lets the embedded
/// UI route relax `default-src 'none'` for its HTML responses without losing the strict
/// default on JSON/API routes.
pub async fn security_headers_middleware(request: Request, next: Next) -> Response<Body> {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    set_if_missing(
        headers,
        HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    set_if_missing(
        headers,
        HeaderName::from_static("x-frame-options"),
        HeaderValue::from_static("DENY"),
    );
    set_if_missing(
        headers,
        HeaderName::from_static("referrer-policy"),
        HeaderValue::from_static("strict-origin-when-cross-origin"),
    );
    set_if_missing(
        headers,
        HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
    );
    set_if_missing(
        headers,
        CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; frame-ancestors 'none'"),
    );
    response
}

fn set_if_missing(headers: &mut axum::http::HeaderMap, name: HeaderName, value: HeaderValue) {
    headers.entry(name).or_insert(value);
}
