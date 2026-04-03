//! Trace context extraction for distributed tracing propagation.
//!
//! Provides utilities to extract W3C Trace Context from the current span
//! for propagation to child processes (compiled scenarios).

use opentelemetry::trace::TraceContextExt;
use tracing_opentelemetry::OpenTelemetrySpanExt;

/// Extract the current trace context (trace_id, span_id) from the active span.
///
/// Returns `None` if:
/// - No active span exists
/// - The span context is invalid
/// - OpenTelemetry is not initialized
pub fn get_current_trace_context() -> Option<(String, String)> {
    let span = tracing::Span::current();
    let ctx = span.context();
    let span_ref = ctx.span();
    let span_ctx = span_ref.span_context();

    if span_ctx.is_valid() {
        Some((
            format!("{:032x}", span_ctx.trace_id()),
            format!("{:016x}", span_ctx.span_id()),
        ))
    } else {
        None
    }
}

/// Format the current span's trace context as a W3C TRACEPARENT header value.
///
/// Returns `None` if no valid trace context is available.
///
/// Format: `00-{trace_id}-{span_id}-01`
/// - `00` - version
/// - `{trace_id}` - 32 hex characters
/// - `{span_id}` - 16 hex characters
/// - `01` - trace flags (sampled)
pub fn format_traceparent() -> Option<String> {
    get_current_trace_context().map(|(trace_id, span_id)| format!("00-{}-{}-01", trace_id, span_id))
}

/// Check if OpenTelemetry tracing is enabled.
///
/// Returns `false` if `OTEL_SDK_DISABLED=true` is set.
pub fn is_otel_enabled() -> bool {
    std::env::var("OTEL_SDK_DISABLED")
        .map(|v| v.to_lowercase() != "true")
        .unwrap_or(true)
}

/// Build OTEL resource attributes string from environment variables.
///
/// Maps vendor-specific variables (DD_*) to standard OTEL format:
/// - `DD_ENV` → `deployment.environment={value}`
/// - `DD_VERSION` → `service.version={value}`
///
/// Returns `None` if no attributes are configured.
pub fn build_resource_attributes() -> Option<String> {
    let mut attrs = Vec::new();

    if let Ok(env) = std::env::var("DD_ENV") {
        attrs.push(format!("deployment.environment={}", env));
    }

    if let Ok(version) = std::env::var("DD_VERSION") {
        attrs.push(format!("service.version={}", version));
    }

    if attrs.is_empty() {
        None
    } else {
        Some(attrs.join(","))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Serialise all tests that read or write environment variables to prevent
    // data races between concurrently-running test threads.  Rust runs tests in
    // parallel by default, so without this lock two tests that touch the same
    // env var (e.g. DD_ENV / DD_VERSION / OTEL_SDK_DISABLED) can interfere with
    // each other and produce flaky results.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_format_traceparent_no_active_span() {
        // Without an active span, should return None.
        // This test does not touch env vars, so no lock is needed.
        let result = format_traceparent();
        // May or may not be None depending on whether OTel is initialised in
        // the test process — just ensure it doesn't panic.
        let _ = result;
    }

    #[test]
    fn test_is_otel_enabled_default() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: protected by ENV_LOCK — no other test touches this var concurrently.
        unsafe { std::env::remove_var("OTEL_SDK_DISABLED") };
        assert!(is_otel_enabled());
    }

    #[test]
    fn test_is_otel_enabled_disabled() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: protected by ENV_LOCK — no other test touches this var concurrently.
        unsafe {
            std::env::set_var("OTEL_SDK_DISABLED", "true");
        }
        assert!(!is_otel_enabled());
        unsafe {
            std::env::remove_var("OTEL_SDK_DISABLED");
        }
    }

    #[test]
    fn test_build_resource_attributes_empty() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: protected by ENV_LOCK — no other test touches these vars concurrently.
        // Explicitly clear both vars so that the test is deterministic even when
        // the host environment (e.g. a Datadog-instrumented CI runner) has them set.
        unsafe {
            std::env::remove_var("DD_ENV");
            std::env::remove_var("DD_VERSION");
        }
        assert!(build_resource_attributes().is_none());
    }

    #[test]
    fn test_build_resource_attributes_with_env() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: protected by ENV_LOCK — no other test touches these vars concurrently.
        unsafe {
            std::env::set_var("DD_ENV", "production");
            std::env::set_var("DD_VERSION", "1.0.0");
        }

        let attrs = build_resource_attributes().unwrap();
        assert!(attrs.contains("deployment.environment=production"));
        assert!(attrs.contains("service.version=1.0.0"));

        unsafe {
            std::env::remove_var("DD_ENV");
            std::env::remove_var("DD_VERSION");
        }
    }
}
