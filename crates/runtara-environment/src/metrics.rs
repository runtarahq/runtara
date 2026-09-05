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

use chrono::{DateTime, Utc};
use opentelemetry::metrics::{Counter, Histogram, Meter};
use opentelemetry::{KeyValue, global};
use runtara_core::persistence::{InstanceCompletionMetrics, InstanceMetricsSink};
use sqlx::PgPool;
use std::sync::OnceLock;

use crate::error::{Error, Result};

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

/// Write resource usage and read back the status the guest reported, in one
/// statement.
///
/// The container monitor needs both, and they are the same row: `RETURNING`
/// makes it one round trip instead of two. `None` means no such instance.
/// Called even when there are no metrics to write, so the caller's crash check
/// always has a status to look at.
pub async fn record_resources_returning_status(
    pool: &PgPool,
    instance_id: &str,
    memory_peak_bytes: Option<u64>,
    cpu_usage_usec: Option<u64>,
) -> Result<Option<(String, Option<String>)>> {
    let row: Option<(String, Option<String>)> = sqlx::query_as(
        "UPDATE instances \
         SET memory_peak_bytes = COALESCE(memory_peak_bytes, $2), \
             cpu_usage_usec = COALESCE(cpu_usage_usec, $3) \
         WHERE instance_id = $1 \
         RETURNING status::TEXT, termination_reason::TEXT",
    )
    .bind(instance_id)
    .bind(memory_peak_bytes.map(|v| v as i64))
    .bind(cpu_usage_usec.map(|v| v as i64))
    .fetch_optional(pool)
    .await
    .map_err(|e| Error::Other(format!("record_resources_returning_status: {e}")))?;

    if row.is_some()
        && (memory_peak_bytes.is_some() || cpu_usage_usec.is_some())
        && let Some(metric) = fetch_completion_metrics(pool, instance_id).await?
    {
        let metrics = workflow_metrics();
        let attributes = metric_attributes(&metric);
        record_resources(metrics, &metric, &attributes);
    }

    Ok(row)
}

/// Store raw stderr captured from the runner, for debugging.
///
/// First writer wins, so a later re-report cannot clobber the output that
/// actually explained the failure.
pub async fn record_instance_stderr(pool: &PgPool, instance_id: &str, stderr: &str) -> Result<()> {
    sqlx::query("UPDATE instances SET stderr = COALESCE(stderr, $2) WHERE instance_id = $1")
        .bind(instance_id)
        .bind(stderr)
        .execute(pool)
        .await
        .map_err(|e| Error::Other(format!("record_instance_stderr: {e}")))?;
    Ok(())
}

#[derive(Debug, sqlx::FromRow)]
struct MetricRow {
    tenant_id: String,
    status: String,
    termination_reason: Option<String>,
    started_at: Option<DateTime<Utc>>,
    finished_at: Option<DateTime<Utc>>,
    memory_peak_bytes: Option<i64>,
    cpu_usage_usec: Option<i64>,
}

async fn fetch_completion_metrics(
    pool: &PgPool,
    instance_id: &str,
) -> Result<Option<InstanceCompletionMetrics>> {
    let row: Option<MetricRow> = sqlx::query_as(
        "SELECT tenant_id, status::text AS status, \
                termination_reason::text AS termination_reason, \
                started_at, finished_at, memory_peak_bytes, cpu_usage_usec \
         FROM instances WHERE instance_id = $1",
    )
    .bind(instance_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| Error::Other(format!("fetch_completion_metrics: {e}")))?;

    Ok(row.map(|row| InstanceCompletionMetrics {
        tenant_id: row.tenant_id,
        status: row.status,
        termination_reason: row.termination_reason,
        started_at: row.started_at,
        finished_at: row.finished_at,
        memory_peak_bytes: row.memory_peak_bytes.and_then(|v| u64::try_from(v).ok()),
        cpu_usage_usec: row.cpu_usage_usec.and_then(|v| u64::try_from(v).ok()),
    }))
}
