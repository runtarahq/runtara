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
}

impl Metrics {
    fn new(meter: Meter) -> Self {
        // Worker metrics
        let worker_executions_total = meter
            .u64_counter("runtara.worker.executions.total")
            .with_description("Total number of scenario executions")
            .build();

        let worker_executions_active = meter
            .i64_up_down_counter("runtara.worker.executions.active")
            .with_description("Currently active scenario executions")
            .build();

        let worker_execution_duration = meter
            .f64_histogram("runtara.worker.execution.duration")
            .with_description("Scenario execution duration in seconds")
            .with_unit("s")
            .build();

        // Compilation metrics
        let compilations_total = meter
            .u64_counter("runtara.compilation.total")
            .with_description("Total number of scenario compilations")
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

        Self {
            meter,
            worker_executions_total,
            worker_executions_active,
            worker_execution_duration,
            compilations_total,
            compilations_active,
            compilation_duration,
            compilation_queue_size,
            trigger_events_total,
            trigger_events_failed,
            trigger_processing_duration,
            http_requests_total,
            http_request_duration,
            db_queries_total,
            db_query_duration,
            db_pool_connections_active,
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
            .with_env_filter(EnvFilter::from_default_env())
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

    // Production default: info level, with sqlx at warn to suppress query logs
    // Debug logs can be enabled via RUST_LOG env var when needed
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn"));

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
