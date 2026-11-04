//! HTTP Request Metrics Middleware
//!
//! Records request count and latency metrics for all HTTP endpoints. Where
//! `RUNTARA_METRICS_TENANT_LABEL` is on, each carries the served tenant the
//! request resolved: the one its path named, else its caller's.

use axum::{body::Body, extract::Request, http::StatusCode, middleware::Next, response::Response};
use opentelemetry::KeyValue;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use crate::middleware::tenant_path::RequestedTenant;
use crate::observability::{Metrics, metrics};

/// Where the auth middleware leaves the tenant it resolved, for the request's
/// metrics. Present only while tenant labels are on.
#[derive(Clone, Default)]
pub struct MetricsTenant(Arc<OnceLock<String>>);

/// Note `tenant` as the tenant `extensions`' request acts for, when its
/// metrics are labelled by tenant. The first noted tenant stands.
pub fn note_tenant(extensions: &axum::http::Extensions, tenant: &str) {
    if let Some(MetricsTenant(slot)) = extensions.get::<MetricsTenant>() {
        let _ = slot.set(tenant.to_owned());
    }
}

/// Middleware that records HTTP request metrics
pub async fn http_metrics_middleware(mut request: Request, next: Next) -> Response<Body> {
    let start = Instant::now();

    // Extract request info before passing to handler
    let method = request.method().to_string();
    let path = request.uri().path().to_string();

    // Normalize path to avoid high cardinality (replace UUIDs and IDs with placeholders)
    let normalized_path = normalize_path(&path);

    // The tenant the path named, else the one auth resolves, when labelled.
    let tenant_label = runtara_environment::pipeline_metrics::tenant_label_enabled();
    let (named, resolved) = if tenant_label {
        let resolved = MetricsTenant::default();
        request.extensions_mut().insert(resolved.clone());
        let named = request
            .extensions()
            .get::<RequestedTenant>()
            .map(|RequestedTenant(tenant)| tenant.clone());
        (named, Some(resolved))
    } else {
        (None, None)
    };

    // Call the next handler
    let response = next.run(request).await;

    // Record metrics
    if let Some(m) = metrics() {
        let tenant = named.or_else(|| resolved.and_then(|MetricsTenant(slot)| slot.get().cloned()));
        let tenant = tenant.and_then(|tenant| crate::observability::tenant_attribute(&tenant));
        record(
            m,
            method,
            normalized_path,
            response.status(),
            start.elapsed().as_secs_f64(),
            tenant,
        );
    }

    response
}

fn record(
    m: &Metrics,
    method: String,
    normalized_path: String,
    status: StatusCode,
    duration: f64,
    tenant: Option<KeyValue>,
) {
    let mut attributes = vec![
        KeyValue::new("method", method.clone()),
        KeyValue::new("path", normalized_path.clone()),
        KeyValue::new("status", status.as_u16().to_string()),
        KeyValue::new("status_class", status_class(status)),
    ];
    attributes.extend(tenant.clone());
    m.http_requests_total.add(1, &attributes);

    let mut duration_attributes = vec![
        KeyValue::new("method", method),
        KeyValue::new("path", normalized_path),
    ];
    duration_attributes.extend(tenant);
    m.http_request_duration
        .record(duration, &duration_attributes);
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

    /// Every HTTP request metric's attribute keys, with the tenant's value.
    fn recorded(enabled: bool, tenant: &str) -> Vec<Vec<String>> {
        use opentelemetry::metrics::MeterProvider;
        use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};
        use opentelemetry_sdk::metrics::{InMemoryMetricExporter, SdkMeterProvider};

        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder()
            .with_periodic_exporter(exporter.clone())
            .build();
        let metrics = Metrics::new(provider.meter("http-test"));
        let served = |t: &str| t == "acme" || t == "beta";
        record(
            &metrics,
            "GET".into(),
            "/api/runtime/workflows".into(),
            StatusCode::OK,
            0.01,
            crate::observability::tenant_attribute_for(enabled, tenant, served),
        );
        provider.force_flush().unwrap();
        let mut points = Vec::new();
        for resource in exporter.get_finished_metrics().unwrap() {
            for scope in resource.scope_metrics() {
                for metric in scope.metrics() {
                    let attributes: Vec<Vec<String>> = match metric.data() {
                        AggregatedMetrics::U64(MetricData::Sum(sum)) => sum
                            .data_points()
                            .map(|p| {
                                p.attributes()
                                    .map(|a| format!("{}={}", a.key, a.value))
                                    .collect()
                            })
                            .collect(),
                        AggregatedMetrics::F64(MetricData::Histogram(h)) => h
                            .data_points()
                            .map(|p| {
                                p.attributes()
                                    .map(|a| format!("{}={}", a.key, a.value))
                                    .collect()
                            })
                            .collect(),
                        _ => continue,
                    };
                    points.extend(attributes);
                }
            }
        }
        provider.shutdown().unwrap();
        points
    }

    #[test]
    fn request_metrics_carry_the_served_tenant_only_when_enabled() {
        let enabled = recorded(true, "acme");
        assert_eq!(enabled.len(), 2, "a count and a duration: {enabled:?}");
        assert!(
            enabled
                .iter()
                .all(|p| p.contains(&"tenant_id=acme".to_string())),
            "{enabled:?}"
        );

        for (on, tenant) in [(false, "acme"), (true, "unserved"), (true, "")] {
            let points = recorded(on, tenant);
            assert_eq!(points.len(), 2);
            assert!(
                points
                    .iter()
                    .flatten()
                    .all(|a| !a.starts_with("tenant_id=")),
                "{on} {tenant:?}: {points:?}"
            );
        }
    }

    #[test]
    fn a_noted_tenant_reaches_the_slot_once_and_only_when_present() {
        let mut extensions = axum::http::Extensions::new();
        note_tenant(&extensions, "acme");
        assert!(extensions.get::<MetricsTenant>().is_none());
        let slot = MetricsTenant::default();
        extensions.insert(slot.clone());
        note_tenant(&extensions, "acme");
        note_tenant(&extensions, "beta");
        assert_eq!(slot.0.get().map(String::as_str), Some("acme"));
    }

    #[test]
    fn test_status_class() {
        assert_eq!(status_class(StatusCode::OK), "2xx");
        assert_eq!(status_class(StatusCode::CREATED), "2xx");
        assert_eq!(status_class(StatusCode::NOT_FOUND), "4xx");
        assert_eq!(status_class(StatusCode::INTERNAL_SERVER_ERROR), "5xx");
    }
}
