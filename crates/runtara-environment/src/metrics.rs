// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! OpenTelemetry reporting for workflow executions, and the forensic columns
//! the runner collects about the process behind one.
//!
//! Core assembles the facts and hands them over through
//! [`InstanceMetricsSink`]; the OTLP vocabulary, the attribute names and the
//! exporter all live here. Resource usage and stderr are written here too:
//! peak memory, CPU time, exit status and captured output are the runner's
//! observations about a process, and Core never reads any of them back to
//! decide anything.

use opentelemetry::metrics::{Counter, Histogram, Meter};
use opentelemetry::{KeyValue, global};
use runtara_core::persistence::{InstanceCompletionMetrics, InstanceMetricsSink};
use sqlx::PgPool;
use std::sync::OnceLock;

use crate::error::Result;
use crate::instance_repository::InstanceRepository;

static WORKFLOW_METRICS: OnceLock<WorkflowMetrics> = OnceLock::new();

struct WorkflowMetrics {
    invocations_total: Counter<u64>,
    execution_duration: Histogram<f64>,
    memory_peak: Histogram<f64>,
    cpu_usage: Histogram<f64>,
}

impl WorkflowMetrics {
    fn new(meter: Meter) -> Self {
        Self {
            invocations_total: meter
                .u64_counter("runtara.workflow.invocations.total")
                .with_description("Total terminal workflow invocations")
                .build(),
            execution_duration: meter
                .f64_histogram("runtara.workflow.execution.duration")
                .with_description("Workflow execution duration in seconds")
                .with_unit("s")
                .build(),
            memory_peak: meter
                .f64_histogram("runtara.workflow.memory.peak")
                .with_description("Workflow peak memory usage in bytes")
                .with_unit("By")
                .build(),
            cpu_usage: meter
                .f64_histogram("runtara.workflow.cpu.usage")
                .with_description("Workflow CPU usage in seconds")
                .with_unit("s")
                .build(),
        }
    }
}

fn workflow_metrics() -> &'static WorkflowMetrics {
    WORKFLOW_METRICS.get_or_init(|| WorkflowMetrics::new(global::meter("runtara-environment")))
}

fn metric_attributes(metric: &InstanceCompletionMetrics) -> Vec<KeyValue> {
    vec![
        KeyValue::new("tenant_id", metric.tenant_id.clone()),
        KeyValue::new("status", crate::core_types::status_name(metric.status)),
        KeyValue::new(
            "termination_reason",
            metric
                .termination_reason
                .clone()
                .unwrap_or_else(|| "none".to_string()),
        ),
    ]
}

fn record_resources(
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

/// Reports Core's terminal-state facts as OTLP workflow metrics.
///
/// Wire it with `PostgresPersistence::with_metrics_sink`. A host that does not
/// is simply not reporting; Core behaves identically either way.
#[derive(Debug, Default, Clone, Copy)]
pub struct OtlpMetricsSink;

impl InstanceMetricsSink for OtlpMetricsSink {
    fn on_terminal(&self, metric: &InstanceCompletionMetrics) {
        let metrics = workflow_metrics();
        let attributes = metric_attributes(metric);

        metrics.invocations_total.add(1, &attributes);
        if let Some(duration_seconds) = metric.duration_seconds() {
            metrics
                .execution_duration
                .record(duration_seconds, &attributes);
        }
        record_resources(metrics, metric, &attributes);
    }
}

/// Record what the process used, report it, and hand back the status the guest
/// reported.
///
/// The persistence half belongs to
/// [`InstanceRepository`](crate::instance_repository::InstanceRepository); what
/// stays here is the OTLP vocabulary, which is this module's whole job.
pub async fn record_resources_returning_status(
    pool: &PgPool,
    instance_id: &str,
    memory_peak_bytes: Option<u64>,
    cpu_usage_usec: Option<u64>,
) -> Result<Option<(runtara_core::domain::InstanceStatus, Option<String>)>> {
    let instances = InstanceRepository::new(pool.clone());
    let observed = instances
        .record_resources_returning_status(instance_id, memory_peak_bytes, cpu_usage_usec)
        .await?;

    if observed.is_some()
        && (memory_peak_bytes.is_some() || cpu_usage_usec.is_some())
        && let Some(metric) = instances.completion_metrics(instance_id).await?
    {
        let metrics = workflow_metrics();
        let attributes = metric_attributes(&metric);
        record_resources(metrics, &metric, &attributes);
    }

    Ok(observed)
}

/// Store raw stderr captured from the runner, for debugging.
pub async fn record_instance_stderr(pool: &PgPool, instance_id: &str, stderr: &str) -> Result<()> {
    InstanceRepository::new(pool.clone())
        .record_stderr(instance_id, stderr)
        .await
}
