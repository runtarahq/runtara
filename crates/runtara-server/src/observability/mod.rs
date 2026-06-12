pub mod db;
pub mod trace_context;

use opentelemetry::KeyValue;
use opentelemetry::global;
use opentelemetry::metrics::{Counter, Histogram, Meter, UpDownCounter};
use opentelemetry::trace::TracerProvider;
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_semantic_conventions as semconv;
use std::sync::OnceLock;
use std::time::Duration;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

/// Production-default `EnvFilter` directives.
///
/// Two things matter here. First, our own crates run at INFO so workflow
/// lifecycle events (`workflow.execute`, `step.*`) are visible. Second,
/// `tracing_subscriber::fmt::FmtSpan::NEW` logs every span CREATION — and
/// wasmtime emits one `compile` + one `translate-to-CLIF` span per wasm
/// function. A single workflow startup has hundreds of these, which floods
/// journald / Datadog with `wasm[0]::function[N] compiled in 0ns` lines that
/// carry no signal. Mute the wasmtime/cranelift targets below WARN so the
/// per-function span fires never reach the formatter.
///
/// Broad `RUST_LOG=debug` style settings still get these mutes appended. Anyone
/// needing the compile traces back can set an explicit target override such as
/// `RUST_LOG=debug,wasmtime=trace`.
pub(super) const DEFAULT_LOG_FILTER: &str = "info,sqlx=warn,wasmtime=warn,\
     wasmtime_cache=warn,wasmtime_cranelift=warn,wasmtime_environ=warn,\
     wasmtime_jit=warn,wasmtime_runtime=warn,wasmtime_wasi=warn,\
     wasmtime_wasi_http=warn,cranelift_codegen=warn,cranelift_control=warn,\
     cranelift_frontend=warn,cranelift_native=warn,cranelift_wasm=warn,\
     regalloc2=warn";

const NOISY_LOG_TARGETS: &[&str] = &[
    "sqlx",
    "wasmtime",
    "wasmtime_cache",
    "wasmtime_cranelift",
    "wasmtime_environ",
    "wasmtime_jit",
    "wasmtime_runtime",
    "wasmtime_wasi",
    "wasmtime_wasi_http",
    "cranelift_codegen",
    "cranelift_control",
    "cranelift_frontend",
    "cranelift_native",
    "cranelift_wasm",
    "regalloc2",
];

fn default_env_filter() -> EnvFilter {
    let filter = std::env::var("RUST_LOG")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|value| effective_log_filter(Some(&value)))
        .unwrap_or_else(|| effective_log_filter(None));

    EnvFilter::try_new(filter).unwrap_or_else(|_| EnvFilter::new(DEFAULT_LOG_FILTER))
}

fn effective_log_filter(rust_log: Option<&str>) -> String {
    match rust_log.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => append_noisy_target_mutes(value),
        None => DEFAULT_LOG_FILTER.to_string(),
    }
}

fn append_noisy_target_mutes(rust_log: &str) -> String {
    let mut filter = rust_log.trim().to_string();
    if !has_broad_directive_at_warn_or_more_verbose(rust_log) {
        return filter;
    }
    for target in NOISY_LOG_TARGETS {
        if filter_mentions_target(rust_log, target) {
            continue;
        }
        if !filter.is_empty() {
            filter.push(',');
        }
        filter.push_str(target);
        filter.push_str("=warn");
    }
    filter
}

fn filter_mentions_target(filter: &str, target: &str) -> bool {
    filter
        .split(',')
        .filter_map(directive_target)
        .any(|directive| directive == target)
}

fn directive_target(directive: &str) -> Option<&str> {
    let directive = directive.trim();
    if directive.is_empty() {
        return None;
    }

    let before_level = directive
        .split_once('=')
        .map_or(directive, |(target, _)| target);
    let target = before_level
        .split_once("[{")
        .map_or(before_level, |(target, _)| target)
        .trim();

    if target.is_empty()
        || matches!(
            target.to_ascii_lowercase().as_str(),
            "off" | "error" | "warn" | "info" | "debug" | "trace"
        )
    {
        None
    } else {
        Some(target)
    }
}

fn has_broad_directive_at_warn_or_more_verbose(filter: &str) -> bool {
    filter.split(',').any(|directive| {
        matches!(
            directive.trim().to_ascii_lowercase().as_str(),
            "warn" | "info" | "debug" | "trace"
        )
    })
}

/// Global metrics instruments
static METRICS: OnceLock<Metrics> = OnceLock::new();

/// Application metrics instruments
pub struct Metrics {
    meter: Meter,

    // Worker metrics
    pub worker_executions_total: Counter<u64>,
    pub worker_executions_active: UpDownCounter<i64>,
    pub worker_execution_duration: Histogram<f64>,

    // Compilation metrics
    pub compilations_total: Counter<u64>,
    pub compilations_active: UpDownCounter<i64>,
    pub compilation_duration: Histogram<f64>,
    pub compilation_queue_size: UpDownCounter<i64>,
    pub direct_compilations_total: Counter<u64>,
    pub direct_compilation_duration: Histogram<f64>,

    // Trigger worker metrics
    pub trigger_events_total: Counter<u64>,
    pub trigger_events_failed: Counter<u64>,
    pub trigger_processing_duration: Histogram<f64>,

    // HTTP request metrics
    pub http_requests_total: Counter<u64>,
    pub http_request_duration: Histogram<f64>,

    // Database metrics
    pub db_queries_total: Counter<u64>,
    pub db_query_duration: Histogram<f64>,
    pub db_pool_connections_active: UpDownCounter<i64>,

    // Agent test metrics. Labeled by `engine` (components|legacy), `agent`,
    // and `capability` so dashboards can compare per-engine latency and
    // throughput during the migration.
    pub agent_test_total: Counter<u64>,
    pub agent_test_failed: Counter<u64>,
    pub agent_test_duration: Histogram<f64>,

    // Auth / authorization metrics. Six conceptual signals (membership lookup
    // latency / failures / missing-member / revoked-token / permission denials
    // by permission / Valkey unavailable) collapse into three instruments
    // because OTEL attributes carry the low-cardinality breakdowns the others
    // would be separate counters for:
    //   - lookup latency                  -> auth_membership_lookup_duration
    //   - lookup failures, missing member,
    //     revoked token, Valkey unavailable-> auth_membership_denials_total{code}
    //   - permission denial by permission -> auth_permission_denials_total{permission}
    // `enforced` distinguishes a real `Required`-mode block from a `Logging`-mode
    // shadow denial; `auth_method` separates the JWT and API-key paths.
    pub auth_membership_lookup_duration: Histogram<f64>,
    pub auth_membership_denials_total: Counter<u64>,
    pub auth_permission_denials_total: Counter<u64>,
}

impl Metrics {
    fn new(meter: Meter) -> Self {
        // Worker metrics
        let worker_executions_total = meter
            .u64_counter("runtara.worker.executions.total")
            .with_description("Total number of workflow executions")
            .build();

        let worker_executions_active = meter
            .i64_up_down_counter("runtara.worker.executions.active")
            .with_description("Currently active workflow executions")
            .build();

        let worker_execution_duration = meter
            .f64_histogram("runtara.worker.execution.duration")
            .with_description("Workflow execution duration in seconds")
            .with_unit("s")
            .build();

        // Compilation metrics
        let compilations_total = meter
            .u64_counter("runtara.compilation.total")
            .with_description("Total number of workflow compilations")
            .build();

        let compilations_active = meter
            .i64_up_down_counter("runtara.compilation.active")
            .with_description("Currently active compilations")
            .build();

        let compilation_duration = meter
            .f64_histogram("runtara.compilation.duration")
            .with_description("Compilation duration in seconds")
            .with_unit("s")
            .build();

        let compilation_queue_size = meter
            .i64_up_down_counter("runtara.compilation.queue.size")
            .with_description("Number of pending compilations in queue")
            .build();

        let direct_compilations_total = meter
            .u64_counter("runtara.compilation.direct.total")
            .with_description("Direct workflow compile attempts by outcome")
            .build();

        let direct_compilation_duration = meter
            .f64_histogram("runtara.compilation.direct.duration")
            .with_description("Direct workflow compile attempt duration in seconds")
            .with_unit("s")
            .build();

        // Trigger worker metrics
        let trigger_events_total = meter
            .u64_counter("runtara.trigger.events.total")
            .with_description("Total trigger events processed")
            .build();

        let trigger_events_failed = meter
            .u64_counter("runtara.trigger.events.failed")
            .with_description("Failed trigger events")
            .build();

        let trigger_processing_duration = meter
            .f64_histogram("runtara.trigger.processing.duration")
            .with_description("Trigger event processing duration in seconds")
            .with_unit("s")
            .build();

        // HTTP request metrics
        let http_requests_total = meter
            .u64_counter("runtara.http.requests.total")
            .with_description("Total HTTP requests")
            .build();

        let http_request_duration = meter
            .f64_histogram("runtara.http.request.duration")
            .with_description("HTTP request duration in seconds")
            .with_unit("s")
            .build();

        // Database metrics
        let db_queries_total = meter
            .u64_counter("runtara.db.queries.total")
            .with_description("Total database queries")
            .build();

        let db_query_duration = meter
            .f64_histogram("runtara.db.query.duration")
            .with_description("Database query duration in seconds")
            .with_unit("s")
            .build();

        let db_pool_connections_active = meter
            .i64_up_down_counter("runtara.db.pool.connections.active")
            .with_description("Active database pool connections")
            .build();

        // Agent test metrics
        let agent_test_total = meter
            .u64_counter("runtara.agent_test.total")
            .with_description("Total agent test invocations")
            .build();

        let agent_test_failed = meter
            .u64_counter("runtara.agent_test.failed")
            .with_description("Failed agent test invocations")
            .build();

        let agent_test_duration = meter
            .f64_histogram("runtara.agent_test.duration")
            .with_description("Agent test invocation duration in seconds")
            .with_unit("s")
            .build();

        // Auth / authorization metrics
        let auth_membership_lookup_duration = meter
            .f64_histogram("runtara.auth.membership.lookup.duration")
            .with_description("Per-tenant Valkey membership/revocation lookup duration in seconds")
            .with_unit("s")
            .build();

        let auth_membership_denials_total = meter
            .u64_counter("runtara.auth.membership.denials.total")
            .with_description(
                "Membership/revocation denials by `code` (NOT_A_MEMBER, TOKEN_REVOKED, \
                 AUTH_MEMBERSHIP_UNAVAILABLE, ...), `auth_method`, and whether `enforced`",
            )
            .build();

        let auth_permission_denials_total = meter
            .u64_counter("runtara.auth.permission.denials.total")
            .with_description("Route-level authorization denials by `permission`")
            .build();

        Self {
            meter,
            worker_executions_total,
            worker_executions_active,
            worker_execution_duration,
            compilations_total,
            compilations_active,
            compilation_duration,
            compilation_queue_size,
            direct_compilations_total,
            direct_compilation_duration,
            trigger_events_total,
            trigger_events_failed,
            trigger_processing_duration,
            http_requests_total,
            http_request_duration,
            db_queries_total,
            db_query_duration,
            db_pool_connections_active,
            agent_test_total,
            agent_test_failed,
            agent_test_duration,
            auth_membership_lookup_duration,
            auth_membership_denials_total,
            auth_permission_denials_total,
        }
    }

    /// Get the underlying meter for creating additional instruments
    pub fn meter(&self) -> &Meter {
        &self.meter
    }
}

/// Get the global metrics instance
pub fn metrics() -> Option<&'static Metrics> {
    METRICS.get()
}

/// Record the duration of a per-tenant membership/revocation lookup. `outcome` is `"allow"`
/// (a role resolved) or `"deny"` (any denial code), `auth_method` the stable identifier from
/// [`crate::auth::AuthMethod::as_str`]. No-op when telemetry is disabled (tests, OTEL off).
pub fn record_membership_lookup(
    duration_secs: f64,
    auth_method: &'static str,
    outcome: &'static str,
) {
    if let Some(m) = metrics() {
        m.auth_membership_lookup_duration.record(
            duration_secs,
            &[
                KeyValue::new("auth_method", auth_method),
                KeyValue::new("outcome", outcome),
            ],
        );
    }
}

/// Count a membership/revocation denial, keyed by its stable `code` (see
/// [`crate::middleware::auth`]), the `auth_method`, and whether enforcement actually blocked
/// the request (`enforced=false` is a `Logging`-mode shadow denial). No-op when telemetry is off.
pub fn record_membership_denial(code: &'static str, auth_method: &'static str, enforced: bool) {
    if let Some(m) = metrics() {
        m.auth_membership_denials_total.add(
            1,
            &[
                KeyValue::new("code", code),
                KeyValue::new("auth_method", auth_method),
                KeyValue::new("enforced", enforced),
            ],
        );
    }
}

/// Count a route-level authorization denial, keyed by the colon-form `permission` that was
/// refused. No-op when telemetry is disabled.
pub fn record_permission_denial(permission: &'static str) {
    if let Some(m) = metrics() {
        m.auth_permission_denials_total
            .add(1, &[KeyValue::new("permission", permission)]);
    }
}

/// Initialize OpenTelemetry with OTLP exporter
///
/// Uses environment variables:
/// - OTEL_EXPORTER_OTLP_ENDPOINT (default: http://localhost:4317)
/// - OTEL_SERVICE_NAME (default: runtara-server)
/// - DD_ENV, DD_SERVICE, DD_VERSION
pub fn init_telemetry() -> Result<(), Box<dyn std::error::Error>> {
    // Check if disabled
    if std::env::var("OTEL_SDK_DISABLED")
        .unwrap_or_else(|_| "false".to_string())
        .to_lowercase()
        == "true"
    {
        tracing_subscriber::fmt()
            .with_env_filter(default_env_filter())
            .with_target(true)
            .with_thread_ids(false)
            .with_file(false)
            .with_line_number(false)
            .with_ansi(false) // Disable colors for journald/Datadog
            .with_span_events(FmtSpan::NEW) // Log when spans are created (shows tenant_id, version)
            .init();
        tracing::info!("OpenTelemetry disabled");
        return Ok(());
    }

    // Get service name from environment
    let service_name = std::env::var("OTEL_SERVICE_NAME")
        .or_else(|_| std::env::var("DD_SERVICE"))
        .unwrap_or_else(|_| "runtara-server".to_string());

    let service_version =
        std::env::var("DD_VERSION").unwrap_or_else(|_| env!("BUILD_VERSION").to_string());

    let environment = std::env::var("DD_ENV").unwrap_or_else(|_| "development".to_string());

    // Create resource with service metadata
    let resource = Resource::builder()
        .with_service_name(service_name.clone())
        .with_attributes([
            KeyValue::new(semconv::resource::SERVICE_VERSION, service_version),
            KeyValue::new(semconv::resource::DEPLOYMENT_ENVIRONMENT_NAME, environment),
        ])
        .build();

    // Create OTLP span exporter for traces
    let trace_exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .build()?;

    // Create tracer provider with resource
    let trace_provider = SdkTracerProvider::builder()
        .with_batch_exporter(trace_exporter)
        .with_resource(resource.clone())
        .build();

    // Set global trace provider
    global::set_tracer_provider(trace_provider.clone());

    // Get tracer
    let tracer = trace_provider.tracer("runtara-server");

    // Create OTLP metrics exporter
    let metrics_exporter = opentelemetry_otlp::MetricExporter::builder()
        .with_tonic()
        .build()?;

    // Create metrics provider with periodic reader (exports every 60 seconds)
    let metrics_reader = PeriodicReader::builder(metrics_exporter)
        .with_interval(Duration::from_secs(60))
        .build();

    let meter_provider = SdkMeterProvider::builder()
        .with_reader(metrics_reader)
        .with_resource(resource.clone())
        .build();

    // Set global meter provider
    global::set_meter_provider(meter_provider);

    // Create and initialize global metrics instruments
    let meter = global::meter("runtara-server");
    let _ = METRICS.set(Metrics::new(meter));

    // Create OTLP log exporter
    let log_exporter = opentelemetry_otlp::LogExporter::builder()
        .with_tonic()
        .build()?;

    let logger_provider = SdkLoggerProvider::builder()
        .with_batch_exporter(log_exporter)
        .with_resource(resource.clone())
        .build();

    // Bridge tracing events to OTLP logs
    let otel_log_layer = OpenTelemetryTracingBridge::new(&logger_provider);

    // Setup OpenTelemetry tracing layer
    let otel_trace_layer = tracing_opentelemetry::layer().with_tracer(tracer);

    let env_filter = default_env_filter();

    tracing_subscriber::registry()
        .with(env_filter)
        .with(otel_trace_layer)
        .with(otel_log_layer)
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(true)
                .with_thread_ids(false)
                .with_file(false)
                .with_line_number(false)
                .with_ansi(false) // Disable colors for journald/Datadog
                .with_span_events(FmtSpan::NEW), // Log when spans are created (shows tenant_id, version)
        )
        .init();

    tracing::info!("OpenTelemetry initialized (traces + metrics + logs)");

    Ok(())
}

pub fn shutdown_telemetry() {
    tracing::info!("Shutting down OpenTelemetry...");
    // Note: In opentelemetry 0.31, providers are shutdown automatically on drop
    // or you need to call shutdown on the provider instance directly
    tracing::info!("OpenTelemetry shutdown complete");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tracing::Subscriber;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::layer::{Context, Layer};

    /// Records every event that makes it past the filter so we can assert
    /// what the production fallback `EnvFilter` admits vs. mutes.
    struct CaptureLayer {
        events: Arc<Mutex<Vec<(String, tracing::Level)>>>,
    }

    impl<S: Subscriber> Layer<S> for CaptureLayer {
        fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
            let m = event.metadata();
            self.events
                .lock()
                .unwrap()
                .push((m.target().to_string(), *m.level()));
        }
    }

    fn capture_events(filter: EnvFilter, emit: impl FnOnce()) -> Vec<(String, tracing::Level)> {
        let captured = Arc::new(Mutex::new(Vec::<(String, tracing::Level)>::new()));
        let layer = CaptureLayer {
            events: captured.clone(),
        };
        let subscriber = tracing_subscriber::registry().with(filter).with(layer);

        tracing::subscriber::with_default(subscriber, emit);

        captured.lock().unwrap().clone()
    }

    /// Asserts that the production-default `EnvFilter` mutes wasmtime/cranelift
    /// noise below WARN while letting our own targets through at INFO.
    ///
    /// Regression for the prod incident where `FmtSpan::NEW` + wasmtime's
    /// per-function compile spans flooded journald (hundreds of
    /// `wasm[0]::function[N] compiled in 0ns` lines per workflow startup).
    /// Without this test, any future change to `DEFAULT_LOG_FILTER` that drops
    /// the wasmtime/cranelift mute directives would silently reintroduce the
    /// flood.
    #[test]
    fn default_filter_mutes_wasmtime_below_warn() {
        let events = capture_events(EnvFilter::new(DEFAULT_LOG_FILTER), || {
            // Wasmtime / cranelift at TRACE/DEBUG/INFO — must be muted.
            tracing::trace!(target: "wasmtime", "compile in 0ns");
            tracing::debug!(target: "wasmtime", "translate to CLIF");
            tracing::info!(target: "wasmtime_cranelift", "function 473");
            tracing::info!(target: "wasmtime_jit", "compiled");
            tracing::info!(target: "wasmtime_wasi", "stdio");
            tracing::info!(target: "cranelift_codegen", "regalloc");
            tracing::info!(target: "cranelift_frontend", "lowering");
            tracing::info!(target: "cranelift_wasm", "translating");
            tracing::debug!(target: "regalloc2", "allocation");
            // At WARN — must pass.
            tracing::warn!(target: "wasmtime", "real wasmtime warning");
            // sqlx at INFO — must be muted (query spam).
            tracing::info!(target: "sqlx", "query result");
            tracing::warn!(target: "sqlx", "slow query");
            // Our crates at INFO — must pass.
            tracing::info!(target: "runtara_server", "server started");
            tracing::info!(target: "runtara_environment", "instance running");
            tracing::info!(target: "runtara_workflows", "compiled workflow");
        });

        let targets: Vec<_> = events.iter().map(|(t, l)| (t.as_str(), *l)).collect();

        // Things that MUST NOT be in the captured set
        for (target, level) in &[
            ("wasmtime", tracing::Level::TRACE),
            ("wasmtime", tracing::Level::DEBUG),
            ("wasmtime", tracing::Level::INFO),
            ("wasmtime_cranelift", tracing::Level::INFO),
            ("wasmtime_jit", tracing::Level::INFO),
            ("wasmtime_wasi", tracing::Level::INFO),
            ("cranelift_codegen", tracing::Level::INFO),
            ("cranelift_frontend", tracing::Level::INFO),
            ("cranelift_wasm", tracing::Level::INFO),
            ("regalloc2", tracing::Level::DEBUG),
            ("sqlx", tracing::Level::INFO),
        ] {
            assert!(
                !targets.contains(&(*target, *level)),
                "{} at {} should be filtered out by DEFAULT_LOG_FILTER, but it passed: {:?}",
                target,
                level,
                targets
            );
        }

        // Things that MUST be in the captured set
        for (target, level) in &[
            ("wasmtime", tracing::Level::WARN),
            ("sqlx", tracing::Level::WARN),
            ("runtara_server", tracing::Level::INFO),
            ("runtara_environment", tracing::Level::INFO),
            ("runtara_workflows", tracing::Level::INFO),
        ] {
            assert!(
                targets.contains(&(*target, *level)),
                "{} at {} should pass DEFAULT_LOG_FILTER, but it was filtered: {:?}",
                target,
                level,
                targets
            );
        }
    }

    #[test]
    fn broad_rust_log_still_mutes_wasmtime_below_warn() {
        let filter = EnvFilter::new(effective_log_filter(Some("debug")));
        let events = capture_events(filter, || {
            tracing::debug!(target: "runtara_server", "app debug should pass");
            tracing::debug!(target: "wasmtime", "wasmtime debug should be muted");
            tracing::debug!(target: "cranelift_codegen", "cranelift debug should be muted");
            tracing::warn!(target: "wasmtime", "wasmtime warn should pass");
        });

        let targets: Vec<_> = events.iter().map(|(t, l)| (t.as_str(), *l)).collect();
        assert!(targets.contains(&("runtara_server", tracing::Level::DEBUG)));
        assert!(!targets.contains(&("wasmtime", tracing::Level::DEBUG)));
        assert!(!targets.contains(&("cranelift_codegen", tracing::Level::DEBUG)));
        assert!(targets.contains(&("wasmtime", tracing::Level::WARN)));
    }

    #[test]
    fn targeted_rust_log_is_not_broadened() {
        assert_eq!(
            effective_log_filter(Some("runtara_server=debug")),
            "runtara_server=debug"
        );
        assert_eq!(effective_log_filter(Some("error")), "error");
        assert_eq!(effective_log_filter(Some("off")), "off");
    }

    /// Sanity-check that explicit target overrides still work — support
    /// requires being able to escalate wasmtime to trace for one-off debugging
    /// without code changes.
    #[test]
    fn explicit_rust_log_target_override_allows_wasmtime_trace() {
        let filter = EnvFilter::new(effective_log_filter(Some("debug,wasmtime=trace")));
        let events = capture_events(filter, || {
            tracing::trace!(target: "wasmtime", "should pass under wasmtime=trace");
            tracing::debug!(target: "wasmtime_cranelift", "still muted without explicit override");
        });
        let targets: Vec<_> = events.iter().map(|(t, l)| (t.as_str(), *l)).collect();

        assert!(targets.contains(&("wasmtime", tracing::Level::TRACE)));
        assert!(!targets.contains(&("wasmtime_cranelift", tracing::Level::DEBUG)));
    }
}
