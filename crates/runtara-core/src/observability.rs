// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! OpenTelemetry metrics for durable workflow execution.
//!
//! Recorded from the persistence layer as instances reach a terminal state.
//! The host owns the global meter provider; core only emits.

use std::sync::OnceLock;

use chrono::{DateTime, Utc};
use opentelemetry::KeyValue;
use opentelemetry::global;
use opentelemetry::metrics::{Counter, Histogram, Meter};

static WORKFLOW_METRICS: OnceLock<WorkflowMetrics> = OnceLock::new();

/// Execution fields collected from the persisted instance row.
#[derive(Debug, Clone)]
pub(crate) struct InstanceCompletionMetrics {
    /// Tenant identifier for the invocation.
    pub(crate) tenant_id: String,
    /// Terminal status: completed, failed, or cancelled.
    pub(crate) status: String,
    /// Optional terminal reason such as timeout or heartbeat_timeout.
    pub(crate) termination_reason: Option<String>,
    /// When execution began.
    pub(crate) started_at: Option<DateTime<Utc>>,
    /// When execution reached a terminal state.
    pub(crate) finished_at: Option<DateTime<Utc>>,
    /// Peak memory collected by the runner cgroup.
    pub(crate) memory_peak_bytes: Option<u64>,
    /// CPU usage collected by the runner cgroup.
    pub(crate) cpu_usage_usec: Option<u64>,
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

/// Match the terminal statuses used by the analytics tenant metrics query.
pub(crate) fn is_recorded_terminal_status(status: &str) -> bool {
    matches!(status, "completed" | "failed" | "cancelled")
}

/// Record count and duration for a terminal workflow invocation.
pub(crate) fn record_instance_completion(metric: &InstanceCompletionMetrics) {
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
pub(crate) fn record_instance_resources(metric: &InstanceCompletionMetrics) {
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
