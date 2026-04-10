use axum::{body::Body, extract::Request, http::HeaderValue, middleware::Next, response::Response};

/// Middleware that adds security headers to every response on the public server.
///
/// Matches the headers previously set by the API gateway's `header_filter_by_lua_block`.
/// HSTS is deliberately omitted — it stays at the TLS termination layer (gateway/LB).
pub async fn security_headers_middleware(request: Request, next: Next) -> Response<Body> {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
    headers.insert(
        "referrer-policy",
        HeaderValue::from_static("strict-origin-when-cross-origin"),
    );
    headers.insert(
        "permissions-policy",
        HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
    );
    headers.insert(
        "content-security-policy",
        HeaderValue::from_static("default-src 'none'; frame-ancestors 'none'"),
    );
    response
}
