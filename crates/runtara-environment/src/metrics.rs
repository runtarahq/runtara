// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Workflow Usage instruments, recorded from the same committed facts as the
//! in-app history. No per-execution telemetry queries or process-global sink.

use opentelemetry::KeyValue;
use opentelemetry::metrics::{Counter, Histogram, Meter};
use sqlx::PgPool;

use crate::error::Result;
use crate::instance_repository::InstanceRepository;

pub(crate) struct WorkflowMetrics {
    invocations_total: Counter<u64>,
    execution_duration: Histogram<f64>,
    memory_peak: Histogram<f64>,
    cpu_usage: Histogram<f64>,
}

impl WorkflowMetrics {
    pub(crate) fn new(meter: Meter) -> Self {
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

impl WorkflowMetrics {
    pub(crate) fn record(&self, fact: &crate::usage::UsageFact) {
        let attributes = [
            KeyValue::new("tenant_id", fact.tenant_id.clone()),
            KeyValue::new("status", fact.status.clone()),
            KeyValue::new(
                "termination_reason",
                fact.termination_reason
                    .clone()
                    .unwrap_or_else(|| "none".into()),
            ),
        ];
        if fact.completion {
            self.invocations_total.add(1, &attributes);
        }
        if let Some(duration) = fact.duration_ms {
            self.execution_duration
                .record(duration / 1000.0, &attributes);
        }
        if let Some(memory) = fact.memory_bytes {
            self.memory_peak.record(memory as f64, &attributes);
        }
        if let Some(cpu) = fact.cpu_usec {
            self.cpu_usage.record(cpu as f64 / 1_000_000.0, &attributes);
        }
    }
}

/// Persist process resources and return the operational status in one query.
/// Usage capture is transactional; the aggregation worker handles reporting.
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
    InstanceRepository::new(pool.clone())
        .record_resources_returning_status(instance_id, memory_peak_bytes, cpu_usage_usec)
        .await
}

/// Store raw stderr captured from the runner, for debugging.
pub async fn record_instance_stderr(pool: &PgPool, instance_id: &str, stderr: &str) -> Result<()> {
    InstanceRepository::new(pool.clone())
        .record_stderr(instance_id, stderr)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry::metrics::MeterProvider;
    use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};
    use opentelemetry_sdk::metrics::{InMemoryMetricExporter, SdkMeterProvider};

    #[test]
    fn usage_facts_export_counts_and_independent_resource_observations() {
        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder()
            .with_periodic_exporter(exporter.clone())
            .build();
        let metrics = WorkflowMetrics::new(provider.meter("usage-test"));
        let mut fact = crate::usage::UsageFact {
            tenant_id: "tenant".into(),
            status: "completed".into(),
            termination_reason: None,
            completion: true,
            export: true,
            duration_ms: Some(2500.0),
            memory_bytes: None,
            cpu_usec: None,
        };
        metrics.record(&fact);
        fact.completion = false;
        fact.duration_ms = None;
        fact.memory_bytes = Some(4096);
        fact.cpu_usec = Some(500_000);
        metrics.record(&fact);
        provider.force_flush().unwrap();
        let exported = exporter.get_finished_metrics().unwrap();
        let mut seen = 0;
        for metric in exported
            .iter()
            .flat_map(|r| r.scope_metrics())
            .flat_map(|s| s.metrics())
        {
            seen += 1;
            match metric.data() {
                AggregatedMetrics::U64(MetricData::Sum(sum)) => {
                    assert_eq!(metric.name(), "runtara.workflow.invocations.total");
                    assert_eq!(sum.data_points().map(|p| p.value()).sum::<u64>(), 1);
                }
                AggregatedMetrics::F64(MetricData::Histogram(histogram)) => {
                    let points: Vec<_> = histogram.data_points().collect();
                    assert_eq!(points.len(), 1);
                    assert_eq!(points[0].count(), 1);
                    let expected = match metric.name() {
                        "runtara.workflow.execution.duration" => 2.5,
                        "runtara.workflow.memory.peak" => 4096.0,
                        "runtara.workflow.cpu.usage" => 0.5,
                        name => panic!("unexpected metric {name}"),
                    };
                    assert_eq!(points[0].sum(), expected);
                    let labels: Vec<_> = points[0].attributes().map(|kv| kv.key.as_str()).collect();
                    assert_eq!(labels.len(), 3);
                    assert!(labels.iter().all(|label| {
                        ["tenant_id", "status", "termination_reason"].contains(label)
                    }));
                }
                data => panic!("unexpected metric type {data:?}"),
            }
        }
        assert_eq!(seen, 4);
        provider.shutdown().unwrap();
    }
}
