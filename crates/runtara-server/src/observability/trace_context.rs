//! Trace context extraction for distributed tracing propagation.
//!
//! Provides utilities to extract W3C Trace Context from the current span
//! for propagation to child processes (compiled workflows).

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

    /// Run `f` under a thread-local subscriber carrying a real OpenTelemetry
    /// tracer, so span contexts are valid regardless of what the rest of the
    /// test binary has installed globally. `with_default` scopes the
    /// subscriber to this thread, so parallel tests are unaffected.
    fn with_otel_subscriber<T>(f: impl FnOnce() -> T) -> T {
        use opentelemetry::trace::TracerProvider as _;
        use opentelemetry_sdk::trace::SdkTracerProvider;
        use tracing_subscriber::layer::SubscriberExt;

        // No exporter: spans are still sampled and assigned real ids, they
        // just go nowhere, which is all these assertions need.
        let provider = SdkTracerProvider::builder().build();
        let subscriber = tracing_subscriber::registry().with(
            tracing_opentelemetry::layer().with_tracer(provider.tracer("trace-context-test")),
        );
        tracing::subscriber::with_default(subscriber, f)
    }

    #[test]
    fn test_format_traceparent_no_active_span() {
        // A subscriber with no OpenTelemetry layer yields no span context, so
        // there is nothing to propagate. Pinning the subscriber makes the
        // answer deterministic instead of depending on whether some other test
        // in this binary happened to initialise OTel first.
        let result = tracing::subscriber::with_default(tracing_subscriber::registry(), || {
            let span = tracing::info_span!("no_otel_layer");
            let _entered = span.enter();
            format_traceparent()
        });
        assert_eq!(result, None);
    }

    #[test]
    fn test_format_traceparent_with_active_span() {
        let (traceparent, context) = with_otel_subscriber(|| {
            let span = tracing::info_span!("outbound");
            let _entered = span.enter();
            (format_traceparent(), get_current_trace_context())
        });

        let traceparent = traceparent.expect("an active sampled span must produce a traceparent");

        // W3C traceparent: version "00", 32-hex trace id, 16-hex span id, and
        // the sampled flag. A child process parses this verbatim, so the field
        // widths matter as much as the values.
        let fields: Vec<&str> = traceparent.split('-').collect();
        assert_eq!(fields.len(), 4, "malformed traceparent {traceparent:?}");
        assert_eq!(fields[0], "00");
        assert_eq!(fields[1].len(), 32, "trace id must be 32 hex chars");
        assert_eq!(fields[2].len(), 16, "span id must be 16 hex chars");
        assert_eq!(fields[3], "01");
        assert!(
            fields[1]
                .chars()
                .chain(fields[2].chars())
                .all(|c| c.is_ascii_hexdigit()),
            "trace and span ids must be lower-hex, got {traceparent:?}"
        );

        // Neither id may be the all-zero "invalid" sentinel.
        assert_ne!(fields[1], "0".repeat(32));
        assert_ne!(fields[2], "0".repeat(16));

        // The header is exactly the pair `get_current_trace_context` reports.
        let (trace_id, span_id) = context.expect("context must be present alongside traceparent");
        assert_eq!(fields[1], trace_id);
        assert_eq!(fields[2], span_id);
    }

    #[test]
    fn test_sibling_spans_share_a_trace_but_not_a_span_id() {
        // Propagation is only useful if the span id actually tracks the span
        // being propagated from; a stale or constant span id would still pass
        // every shape check above.
        let (outer, inner) = with_otel_subscriber(|| {
            let outer_span = tracing::info_span!("outer");
            let _outer = outer_span.enter();
            let outer_ctx = get_current_trace_context();

            let inner_span = tracing::info_span!("inner");
            let _inner = inner_span.enter();
            let inner_ctx = get_current_trace_context();

            (outer_ctx, inner_ctx)
        });

        let (outer_trace, outer_span) = outer.expect("outer span context");
        let (inner_trace, inner_span) = inner.expect("inner span context");

        assert_eq!(
            outer_trace, inner_trace,
            "a child span joins its parent trace"
        );
        assert_ne!(outer_span, inner_span, "each span needs its own span id");
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
