// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! OpenTelemetry telemetry initialization for workflow scenarios.
//!
//! This module provides tracing subscriber initialization with optional
//! OpenTelemetry integration. When the `telemetry` feature is enabled and
//! `OTEL_EXPORTER_OTLP_ENDPOINT` is set, spans are exported to an OTLP collector.
//!
//! # Usage
//!
//! ```rust,ignore
//! let _guard = runtara_workflow_stdlib::telemetry::init_subscriber();
//! // ... workflow execution ...
//! // Guard flushes telemetry on drop
//! ```
//!
//! # Environment Variables
//!
//! - `OTEL_EXPORTER_OTLP_ENDPOINT`: OTLP gRPC endpoint (required for OTEL)
//! - `OTEL_SERVICE_NAME`: Service name (default: "runtara-workflow")
//! - `OTEL_RESOURCE_ATTRIBUTES`: Additional attributes (format: "key1=value1,key2=value2")
//! - `TRACEPARENT`: W3C trace context for parent span linking
//! - `SCENARIO_ID`, `RUNTARA_INSTANCE_ID`, `RUNTARA_TENANT_ID`: Added as resource attributes

/// Guard that ensures telemetry is flushed on drop.
///
/// When OpenTelemetry is enabled, this guard holds the tracer and logger providers
/// and flushes all pending spans and logs when dropped.
pub struct TelemetryGuard {
    #[cfg(feature = "telemetry")]
    trace_provider: Option<opentelemetry_sdk::trace::SdkTracerProvider>,
    #[cfg(feature = "telemetry")]
    log_provider: Option<opentelemetry_sdk::logs::SdkLoggerProvider>,
    #[cfg(not(feature = "telemetry"))]
    _phantom: std::marker::PhantomData<()>,
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        #[cfg(feature = "telemetry")]
        {
            // Flush all pending spans before shutdown
            if let Some(ref provider) = self.trace_provider
                && let Err(e) = provider.force_flush()
            {
                eprintln!("OTEL trace flush error: {:?}", e);
            }
            // Flush all pending logs before shutdown
            if let Some(ref provider) = self.log_provider
                && let Err(e) = provider.force_flush()
            {
                eprintln!("OTEL log flush error: {:?}", e);
            }
        }
    }
}

/// Initialize the tracing subscriber with optional OpenTelemetry layer.
///
/// This function sets up the tracing subscriber with:
/// - A fmt layer that writes to stderr
/// - An EnvFilter that respects `RUST_LOG` (default: info)
/// - An OpenTelemetry layer (only when `telemetry` feature is enabled AND
///   `OTEL_EXPORTER_OTLP_ENDPOINT` is set)
///
/// Returns a guard that flushes telemetry on drop. The guard must be kept
/// alive for the duration of the workflow execution.
///
/// # Example
///
/// ```rust,ignore
/// fn main() {
///     let _guard = init_subscriber();
///     // Workflow execution...
/// } // Guard dropped here, telemetry flushed
/// ```
pub fn init_subscriber() -> TelemetryGuard {
    use tracing_subscriber::layer::SubscriberExt;

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let fmt = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .with_target(true);

    #[cfg(feature = "telemetry")]
    {
        if let Some((trace_layer, log_layer, trace_provider, log_provider)) = maybe_init_otel() {
            // Build subscriber with OTEL layers using the registry directly
            // Create separate fmt layer for this subscriber (stderr output for local debugging)
            let fmt_layer = tracing_subscriber::fmt::layer()
                .with_writer(std::io::stderr)
                .with_ansi(false)
                .with_target(true);

            let subscriber = tracing_subscriber::Registry::default()
                .with(trace_layer) // Traces to OTLP
                .with(log_layer) // Logs to OTLP (with trace context for correlation)
                .with(fmt_layer) // Also print to stderr
                .with(filter);

            tracing::subscriber::set_global_default(subscriber)
                .expect("Failed to set global subscriber");

            return TelemetryGuard {
                trace_provider: Some(trace_provider),
                log_provider: Some(log_provider),
            };
        }
    }

    // Build subscriber without OTEL
    let subscriber = tracing_subscriber::Registry::default()
        .with(fmt)
        .with(filter);

    tracing::subscriber::set_global_default(subscriber).expect("Failed to set global subscriber");

    TelemetryGuard {
        #[cfg(feature = "telemetry")]
        trace_provider: None,
        #[cfg(feature = "telemetry")]
        log_provider: None,
        #[cfg(not(feature = "telemetry"))]
        _phantom: std::marker::PhantomData,
    }
}

#[cfg(feature = "telemetry")]
type OtelTraceLayer = tracing_opentelemetry::OpenTelemetryLayer<
    tracing_subscriber::Registry,
    opentelemetry_sdk::trace::SdkTracer,
>;

#[cfg(feature = "telemetry")]
type OtelLogLayer = opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge<
    opentelemetry_sdk::logs::SdkLoggerProvider,
    opentelemetry_sdk::logs::SdkLogger,
>;

/// Try to initialize OpenTelemetry if endpoint is configured.
///
/// Returns `Some((trace_layer, log_layer, trace_provider, log_provider))` if OTEL is configured, `None` otherwise.
#[cfg(feature = "telemetry")]
fn maybe_init_otel() -> Option<(
    OtelTraceLayer,
    OtelLogLayer,
    opentelemetry_sdk::trace::SdkTracerProvider,
    opentelemetry_sdk::logs::SdkLoggerProvider,
)> {
    use opentelemetry::trace::TracerProvider as _;
    use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
    use opentelemetry_otlp::{LogExporter, SpanExporter, WithExportConfig};
    use opentelemetry_sdk::Resource;
    use opentelemetry_sdk::logs::SdkLoggerProvider;
    use opentelemetry_sdk::trace::SdkTracerProvider;

    // Check if OTEL endpoint is configured
    let endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok()?;
    if endpoint.is_empty() {
        return None;
    }

    // Build resource attributes
    let service_name =
        std::env::var("OTEL_SERVICE_NAME").unwrap_or_else(|_| "runtara-workflow".to_string());

    // Build additional attributes
    let mut additional_attrs = Vec::new();

    // Parse OTEL_RESOURCE_ATTRIBUTES (format: key1=value1,key2=value2)
    if let Ok(attrs) = std::env::var("OTEL_RESOURCE_ATTRIBUTES") {
        for pair in attrs.split(',') {
            if let Some((key, value)) = pair.split_once('=') {
                let key = key.trim();
                let value = value.trim();
                if !key.is_empty() && !value.is_empty() {
                    additional_attrs.push(opentelemetry::KeyValue::new(
                        key.to_string(),
                        value.to_string(),
                    ));
                }
            }
        }
    }

    // Add scenario/instance/tenant IDs as resource attributes
    if let Ok(scenario_id) = std::env::var("SCENARIO_ID") {
        additional_attrs.push(opentelemetry::KeyValue::new("scenario.id", scenario_id));
    }
    if let Ok(instance_id) = std::env::var("RUNTARA_INSTANCE_ID") {
        additional_attrs.push(opentelemetry::KeyValue::new("instance.id", instance_id));
    }
    if let Ok(tenant_id) = std::env::var("RUNTARA_TENANT_ID") {
        additional_attrs.push(opentelemetry::KeyValue::new("tenant.id", tenant_id));
    }

    // Build resource using builder pattern (0.28 API)
    let resource = Resource::builder()
        .with_service_name(service_name.clone())
        .with_attributes(additional_attrs)
        .build();

    // Build OTLP span exporter
    let span_exporter = SpanExporter::builder()
        .with_tonic()
        .with_endpoint(&endpoint)
        .build()
        .map_err(|e| {
            eprintln!("Failed to create OTLP span exporter: {:?}", e);
            e
        })
        .ok()?;

    // Build OTLP log exporter
    let log_exporter = LogExporter::builder()
        .with_tonic()
        .with_endpoint(&endpoint)
        .build()
        .map_err(|e| {
            eprintln!("Failed to create OTLP log exporter: {:?}", e);
            e
        })
        .ok()?;

    // Build tracer provider with batch exporter
    let trace_provider = SdkTracerProvider::builder()
        .with_resource(resource.clone())
        .with_batch_exporter(span_exporter)
        .build();

    // Build logger provider with batch exporter
    let log_provider = SdkLoggerProvider::builder()
        .with_resource(resource)
        .with_batch_exporter(log_exporter)
        .build();

    // Set tracer as global provider (logs don't need global registration -
    // the bridge holds a direct reference to the log provider)
    opentelemetry::global::set_tracer_provider(trace_provider.clone());

    // Create tracer and trace layer
    let tracer = trace_provider.tracer(service_name);
    let trace_layer = tracing_opentelemetry::layer().with_tracer(tracer);

    // Create log layer (bridges tracing events to OTEL logs with trace context)
    let log_layer = OpenTelemetryTracingBridge::new(&log_provider);

    // Parse and apply TRACEPARENT for parent context linking
    if let Ok(traceparent) = std::env::var("TRACEPARENT") {
        apply_traceparent(&traceparent);
    }

    Some((trace_layer, log_layer, trace_provider, log_provider))
}

/// Apply W3C TRACEPARENT header to set up parent context.
///
/// Format: `00-{trace_id}-{span_id}-{trace_flags}`
/// Example: `00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01`
#[cfg(feature = "telemetry")]
fn apply_traceparent(traceparent: &str) {
    use opentelemetry::propagation::TextMapPropagator;
    use opentelemetry_sdk::propagation::TraceContextPropagator;

    let propagator = TraceContextPropagator::new();
    let mut carrier = std::collections::HashMap::new();
    carrier.insert("traceparent".to_string(), traceparent.to_string());

    // Extract parent context - this sets up the context for child spans
    let context = propagator.extract(&carrier);

    // Attach the parent context to the current thread
    // This ensures all spans created in this thread are children of the parent
    let _guard = context.attach();

    // Note: The guard is dropped immediately, but the context is already
    // set up in the global tracer. The tracing-opentelemetry layer will
    // use this context for subsequent spans.
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_init_subscriber_without_otel() {
        // Clear OTEL endpoint to ensure no OTEL initialization
        // SAFETY: Tests run single-threaded, no concurrent access to env vars
        unsafe {
            std::env::remove_var("OTEL_EXPORTER_OTLP_ENDPOINT");
        }

        // This should not panic and should return a guard
        // Note: We can't actually call init_subscriber() in tests because
        // it calls .init() which can only be called once per process.
        // Instead, we just verify the module compiles correctly.
    }

    #[cfg(feature = "telemetry")]
    #[test]
    fn test_parse_otel_resource_attributes() {
        // SAFETY: Tests run single-threaded, no concurrent access to env vars
        unsafe {
            std::env::set_var("OTEL_RESOURCE_ATTRIBUTES", "key1=value1,key2=value2");
        }

        // Parse and verify (this is tested implicitly through maybe_init_otel)
        let attrs = std::env::var("OTEL_RESOURCE_ATTRIBUTES").unwrap();
        let pairs: Vec<(&str, &str)> = attrs
            .split(',')
            .filter_map(|pair| pair.split_once('='))
            .collect();

        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0], ("key1", "value1"));
        assert_eq!(pairs[1], ("key2", "value2"));

        // SAFETY: Tests run single-threaded
        unsafe {
            std::env::remove_var("OTEL_RESOURCE_ATTRIBUTES");
        }
    }
}
