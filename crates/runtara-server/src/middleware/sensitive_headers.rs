//! Marks credential-bearing headers as sensitive before anything can log them.
//!
//! A [`HeaderValue`](axum::http::HeaderValue) marked sensitive prints as
//! `Sensitive` under `Debug`, so request logging, `TraceLayer` spans with
//! headers and any `?headers` field show the header's name but never its
//! value. Wrap each server's finished router in [`layer`] as its outermost
//! layer, so the marking happens before any other middleware sees the request.

use axum::http::{HeaderName, header};
use tower_http::sensitive_headers::SetSensitiveHeadersLayer;

/// Request and response headers whose values are credentials.
pub fn sensitive_header_names() -> Vec<HeaderName> {
    vec![
        header::AUTHORIZATION,
        header::PROXY_AUTHORIZATION,
        header::COOKIE,
        header::SET_COOKIE,
        HeaderName::from_static("x-api-key"),
        HeaderName::from_static("x-goog-api-key"),
    ]
}

/// The layer that marks [`sensitive_header_names`] on requests and responses.
pub fn layer() -> SetSensitiveHeadersLayer {
    SetSensitiveHeadersLayer::new(sensitive_header_names())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, body::Body, extract::Request, http::HeaderMap, routing::get};
    use std::sync::{Arc, Mutex};
    use tower::ServiceExt;

    const TOKEN: &str = "rt_0123456789abcdef_secret";

    #[tokio::test]
    async fn authorization_value_never_appears_in_debug_output() {
        let seen = Arc::new(Mutex::new(String::new()));
        let sink = seen.clone();
        let app = Router::new()
            .route(
                "/",
                get(move |headers: HeaderMap| {
                    let sink = sink.clone();
                    async move {
                        *sink.lock().unwrap() = format!("{headers:?}");
                    }
                }),
            )
            .layer(layer());
        let request = Request::get("/")
            .header("authorization", format!("Bearer {TOKEN}"))
            .header("x-api-key", TOKEN)
            .header("cookie", format!("session={TOKEN}"))
            .header("x-request-id", "visible")
            .body(Body::empty())
            .unwrap();
        app.oneshot(request).await.unwrap();
        let printed = seen.lock().unwrap().clone();
        // Failure messages never print the captured output, which would carry
        // the credential if it leaked.
        assert!(
            !printed.contains(TOKEN),
            "a credential header value was printed"
        );
        assert!(
            printed.contains("authorization"),
            "the header names were not printed"
        );
        assert!(
            printed.contains("visible"),
            "a plain header value was hidden"
        );
    }
}
