//! HTTP Request Metrics Middleware
//!
//! Records request count and latency metrics for all HTTP endpoints.

use axum::{body::Body, extract::Request, http::StatusCode, middleware::Next, response::Response};
use opentelemetry::KeyValue;
use std::time::Instant;

use crate::observability::metrics;

/// Middleware that records HTTP request metrics
pub async fn http_metrics_middleware(request: Request, next: Next) -> Response<Body> {
    let start = Instant::now();

    // Extract request info before passing to handler
    let method = request.method().to_string();
    let path = request.uri().path().to_string();

    // Normalize path to avoid high cardinality (replace UUIDs and IDs with placeholders)
    let normalized_path = normalize_path(&path);

    // Call the next handler
    let response = next.run(request).await;

    // Record metrics
    let duration = start.elapsed().as_secs_f64();
    let status = response.status().as_u16().to_string();
    let status_class = status_class(response.status());

    if let Some(m) = metrics() {
        let attributes = [
            KeyValue::new("method", method.clone()),
            KeyValue::new("path", normalized_path.clone()),
            KeyValue::new("status", status),
            KeyValue::new("status_class", status_class),
        ];

        m.http_requests_total.add(1, &attributes);
        m.http_request_duration.record(
            duration,
            &[
                KeyValue::new("method", method),
                KeyValue::new("path", normalized_path),
            ],
        );
    }

    response
}

/// Normalize path by replacing dynamic segments with placeholders
fn normalize_path(path: &str) -> String {
    let segments: Vec<&str> = path.split('/').collect();
    let normalized: Vec<String> = segments
        .into_iter()
        .map(|segment| {
            // Replace UUIDs (36 chars with hyphens) or numeric IDs
            if (segment.len() == 36 && segment.chars().filter(|c| *c == '-').count() == 4)
                || (segment.chars().all(|c| c.is_ascii_digit()) && !segment.is_empty())
            {
                "{id}".to_string()
            }
            // Replace version numbers like "v1", "v2", etc.
            else if segment.starts_with('v') && segment[1..].chars().all(|c| c.is_ascii_digit()) {
                "{version}".to_string()
            } else {
                segment.to_string()
            }
        })
        .collect();

    normalized.join("/")
}

/// Get status class (2xx, 3xx, 4xx, 5xx)
fn status_class(status: StatusCode) -> &'static str {
    match status.as_u16() {
        100..=199 => "1xx",
        200..=299 => "2xx",
        300..=399 => "3xx",
        400..=499 => "4xx",
        500..=599 => "5xx",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_path() {
        assert_eq!(
            normalize_path("/api/runtime/workflows/123e4567-e89b-12d3-a456-426614174000"),
            "/api/runtime/workflows/{id}"
        );
        assert_eq!(
            normalize_path(
                "/api/runtime/workflows/123e4567-e89b-12d3-a456-426614174000/versions/5"
            ),
            "/api/runtime/workflows/{id}/versions/{id}"
        );
        assert_eq!(
            normalize_path("/api/runtime/metrics/tenant"),
            "/api/runtime/metrics/tenant"
        );
        assert_eq!(normalize_path("/health"), "/health");
    }

    #[test]
    fn test_status_class() {
        assert_eq!(status_class(StatusCode::OK), "2xx");
        assert_eq!(status_class(StatusCode::CREATED), "2xx");
        assert_eq!(status_class(StatusCode::NOT_FOUND), "4xx");
        assert_eq!(status_class(StatusCode::INTERNAL_SERVER_ERROR), "5xx");
    }
}
