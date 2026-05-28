// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! OpenTelemetry metrics for durable workflow execution.

use std::sync::OnceLock;
use std::time::Duration;

use chrono::{DateTime, Utc};
use opentelemetry::KeyValue;
use opentelemetry::global;
use opentelemetry::metrics::{Counter, Histogram, Meter};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};

static WORKFLOW_METRICS: OnceLock<WorkflowMetrics> = OnceLock::new();

/// Execution fields collected from the persisted instance row.
#[derive(Debug, Clone)]
pub struct InstanceCompletionMetrics {
    /// Tenant identifier for the invocation.
    pub tenant_id: String,
    /// Terminal status: completed, failed, or cancelled.
    pub status: String,
    /// Optional terminal reason such as timeout or heartbeat_timeout.
    pub termination_reason: Option<String>,
    /// When execution began.
    pub started_at: Option<DateTime<Utc>>,
    /// When execution reached a terminal state.
    pub finished_at: Option<DateTime<Utc>>,
    /// Peak memory collected by the runner cgroup.
    pub memory_peak_bytes: Option<u64>,
    /// CPU usage collected by the runner cgroup.
    pub cpu_usage_usec: Option<u64>,
}

impl InstanceCompletionMetrics {
    fn duration_seconds(&self) -> Option<f64> {
        let started_at = self.started_at?;
        let finished_at = self.finished_at?;
        let duration = finished_at.signed_duration_since(started_at);
        duration.to_std().ok().map(|d| d.as_secs_f64())
    }
}

struct WorkflowMetrics {
    invocations_total: Counter<u64>,
    execution_duration: Histogram<f64>,
    memory_peak: Histogram<f64>,
    cpu_usage: Histogram<f64>,
}

impl WorkflowMetrics {
    fn new(meter: Meter) -> Self {
        let invocations_total = meter
            .u64_counter("runtara.workflow.invocations.total")
            .with_description("Total terminal workflow invocations")
            .build();

        let execution_duration = meter
            .f64_histogram("runtara.workflow.execution.duration")
            .with_description("Workflow execution duration in seconds")
            .with_unit("s")
            .build();

        let memory_peak = meter
            .f64_histogram("runtara.workflow.memory.peak")
            .with_description("Workflow peak memory usage in bytes")
            .with_unit("By")
            .build();

        let cpu_usage = meter
            .f64_histogram("runtara.workflow.cpu.usage")
            .with_description("Workflow CPU usage in seconds")
            .with_unit("s")
            .build();

        Self {
            invocations_total,
            execution_duration,
            memory_peak,
            cpu_usage,
        }
    }
}

/// Initialize the OTLP metrics exporter for standalone runtara-core or
/// runtara-environment processes. Embedded runtara-server initializes its own
/// global OpenTelemetry provider before Core starts, so callers should not use
/// this from that path.
pub fn init_metrics_telemetry(
    default_service_name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    if is_otel_disabled() {
        return Ok(());
    }

    let service_name = std::env::var("OTEL_SERVICE_NAME")
        .or_else(|_| std::env::var("DD_SERVICE"))
        .unwrap_or_else(|_| default_service_name.to_string());
    let service_version = std::env::var("OTEL_SERVICE_VERSION")
        .or_else(|_| std::env::var("DD_VERSION"))
        .unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_string());
    let environment = std::env::var("OTEL_DEPLOYMENT_ENVIRONMENT")
        .or_else(|_| std::env::var("DD_ENV"))
        .unwrap_or_else(|_| "development".to_string());

    let resource = Resource::builder()
        .with_service_name(service_name)
        .with_attributes([
            KeyValue::new("service.version", service_version),
            KeyValue::new("deployment.environment.name", environment),
        ])
        .build();

    let metrics_exporter = opentelemetry_otlp::MetricExporter::builder()
        .with_tonic()
        .build()?;
    let metrics_reader = PeriodicReader::builder(metrics_exporter)
        .with_interval(Duration::from_secs(60))
        .build();

    let meter_provider = SdkMeterProvider::builder()
        .with_reader(metrics_reader)
        .with_resource(resource)
        .build();

    global::set_meter_provider(meter_provider);
    Ok(())
}

fn is_otel_disabled() -> bool {
    std::env::var("OTEL_SDK_DISABLED")
        .map(|value| value.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// Match the terminal statuses used by the analytics tenant metrics query.
pub fn is_recorded_terminal_status(status: &str) -> bool {
    matches!(status, "completed" | "failed" | "cancelled")
}

/// Record count and duration for a terminal workflow invocation.
pub fn record_instance_completion(metric: &InstanceCompletionMetrics) {
    if !is_recorded_terminal_status(&metric.status) {
        return;
    }

    let metrics = workflow_metrics();
    let attributes = metric_attributes(metric);

    metrics.invocations_total.add(1, &attributes);

    if let Some(duration_seconds) = metric.duration_seconds() {
        metrics
            .execution_duration
            .record(duration_seconds, &attributes);
    }

    record_resource_metrics_with_attributes(metrics, metric, &attributes);
}

/// Record resource metrics collected after process exit.
pub fn record_instance_resources(metric: &InstanceCompletionMetrics) {
    if !is_recorded_terminal_status(&metric.status) {
        return;
    }

    let metrics = workflow_metrics();
    let attributes = metric_attributes(metric);
    record_resource_metrics_with_attributes(metrics, metric, &attributes);
}

fn workflow_metrics() -> &'static WorkflowMetrics {
    WORKFLOW_METRICS.get_or_init(|| WorkflowMetrics::new(global::meter("runtara-core")))
}

fn metric_attributes(metric: &InstanceCompletionMetrics) -> Vec<KeyValue> {
    vec![
        KeyValue::new("tenant_id", metric.tenant_id.clone()),
        KeyValue::new("status", metric.status.clone()),
        KeyValue::new(
            "termination_reason",
            metric
                .termination_reason
                .clone()
                .unwrap_or_else(|| "none".to_string()),
        ),
    ]
}

fn record_resource_metrics_with_attributes(
    metrics: &WorkflowMetrics,
    metric: &InstanceCompletionMetrics,
    attributes: &[KeyValue],
) {
    if let Some(memory_peak_bytes) = metric.memory_peak_bytes {
        metrics
            .memory_peak
            .record(memory_peak_bytes as f64, attributes);
    }

    if let Some(cpu_usage_usec) = metric.cpu_usage_usec {
        metrics
            .cpu_usage
            .record(cpu_usage_usec as f64 / 1_000_000.0, attributes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorded_terminal_statuses_match_analytics_query() {
        assert!(is_recorded_terminal_status("completed"));
        assert!(is_recorded_terminal_status("failed"));
        assert!(is_recorded_terminal_status("cancelled"));
        assert!(!is_recorded_terminal_status("suspended"));
        assert!(!is_recorded_terminal_status("running"));
    }

    #[test]
    fn duration_uses_started_and_finished_times() {
        let started_at = DateTime::from_timestamp(1_000, 0).unwrap();
        let finished_at = DateTime::from_timestamp(1_001, 500_000_000).unwrap();
        let metric = InstanceCompletionMetrics {
            tenant_id: "tenant".to_string(),
            status: "completed".to_string(),
            termination_reason: None,
            started_at: Some(started_at),
            finished_at: Some(finished_at),
            memory_peak_bytes: None,
            cpu_usage_usec: None,
        };

        assert_eq!(metric.duration_seconds(), Some(1.5));
    }
}
