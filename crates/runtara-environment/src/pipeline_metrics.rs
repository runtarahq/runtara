// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Process-local pipeline instruments. No sampling task, database, or exporter.
//!
//! Owners register their existing semaphores once. Collection only upgrades
//! weak references and reads atomics. Transition counters are recorded from
//! committed operation results; they are not durable queue-length estimates.

use std::sync::{Arc, OnceLock};
use std::time::Instant;

use opentelemetry::metrics::{Counter, Histogram, Meter};
use opentelemetry::{KeyValue, global};
use tokio::sync::Semaphore;

use crate::launch_queue::LaunchState;

/// Read at owner construction, never on an individual event's hot path.
pub fn telemetry_enabled() -> bool {
    !std::env::var("OTEL_SDK_DISABLED").is_ok_and(|value| value.eq_ignore_ascii_case("true"))
}

/// Register additive local capacity and occupancy. Call once per pool owner.
/// The SDK retains callbacks, but callbacks must not keep the pool alive.
pub fn observe_pool(meter: &Meter, pool: &'static str, permits: &Arc<Semaphore>, capacity: usize) {
    let usage = Arc::downgrade(permits);
    meter
        .i64_observable_up_down_counter("runtara.pipeline.pool.usage")
        .with_description("Occupied process-local permits")
        .with_unit("{permit}")
        .with_callback(move |observer| {
            if let Some(permits) = usage.upgrade() {
                observer.observe(
                    capacity.saturating_sub(permits.available_permits()) as i64,
                    &[KeyValue::new("pool", pool)],
                );
            }
        })
        .build();
    let limit = Arc::downgrade(permits);
    meter
        .i64_observable_up_down_counter("runtara.pipeline.pool.capacity")
        .with_description("Configured process-local permit capacity")
        .with_unit("{permit}")
        .with_callback(move |observer| {
            if limit.upgrade().is_some() {
                observer.observe(capacity as i64, &[KeyValue::new("pool", pool)]);
            }
        })
        .build();
}

/// Fixed buckets shared by pipeline durations, from milliseconds to an hour.
pub fn duration_boundaries() -> Vec<f64> {
    vec![
        0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0, 10.0, 30.0, 60.0, 300.0, 3600.0,
    ]
}

/// Instruments shared by local execution owners.
#[derive(Clone, Debug)]
pub struct PipelineMetrics {
    meter: Meter,
    transitions: Counter<u64>,
    queue_duration: Histogram<f64>,
    hold_duration: Histogram<f64>,
    runs: Counter<u64>,
}

impl PipelineMetrics {
    /// Construct instruments on an already initialized provider.
    pub fn new(meter: Meter) -> Self {
        Self {
            transitions: meter
                .u64_counter("runtara.launch.transitions")
                .with_description("Committed launch transitions, excluding idempotent replays")
                .build(),
            queue_duration: meter
                .f64_histogram("runtara.launch.queue.duration")
                .with_description(
                    "Launch creation to confirmed handoff or pre-start terminal outcome",
                )
                .with_unit("s")
                .with_boundaries(duration_boundaries())
                .build(),
            hold_duration: meter
                .f64_histogram("runtara.pipeline.pool.hold.duration")
                .with_description("Duration of a local permit hold, including failed attempts")
                .with_unit("s")
                .with_boundaries(duration_boundaries())
                .build(),
            runs: meter
                .u64_counter("runtara.runner.runs")
                .with_description(
                    "Physical runner slots acquired and released, not logical completions",
                )
                .build(),
            meter,
        }
    }

    /// Process instruments, absent when telemetry is disabled at startup.
    pub fn global() -> Option<&'static Self> {
        static METRICS: OnceLock<Option<PipelineMetrics>> = OnceLock::new();
        METRICS
            .get_or_init(|| {
                telemetry_enabled()
                    .then(|| Self::new(global::meter("runtara-environment.pipeline")))
            })
            .as_ref()
    }

    /// Register a local semaphore once, without retaining it.
    pub fn observe_pool(&self, pool: &'static str, permits: &Arc<Semaphore>, capacity: usize) {
        observe_pool(&self.meter, pool, permits, capacity);
    }

    /// Observe the existing reaper count without keeping the owner alive.
    pub fn observe_reaping(&self, count: &Arc<std::sync::atomic::AtomicU64>) {
        let count = Arc::downgrade(count);
        self.meter
            .i64_observable_up_down_counter("runtara.precompile.children.reaping")
            .with_callback(move |observer| {
                if let Some(count) = count.upgrade() {
                    observer.observe(count.load(std::sync::atomic::Ordering::Relaxed) as i64, &[]);
                }
            })
            .build();
    }

    /// Call only after the transition committed and only for rows it changed.
    pub fn transition(&self, state: LaunchState, reason: &'static str) {
        self.transitions.add(
            1,
            &[
                KeyValue::new("state", state.as_str()),
                KeyValue::new("reason", reason),
            ],
        );
    }

    /// Record the final queue residence from authoritative transition timestamps.
    pub fn queue_duration(&self, seconds: f64, outcome: &'static str) {
        self.queue_duration
            .record(seconds.max(0.0), &[KeyValue::new("outcome", outcome)]);
    }

    /// Start a timer owned by a held permit.
    pub fn hold(&self, pool: &'static str) -> PoolHold {
        if pool == "run" {
            self.runs.add(1, &[KeyValue::new("event", "started")]);
        }
        PoolHold {
            started: Instant::now(),
            pool,
            metrics: self.clone(),
        }
    }
}

/// Owned by the actual permit guard, so cancellation and unwind record a stop.
#[derive(Debug)]
pub struct PoolHold {
    started: Instant,
    pool: &'static str,
    metrics: PipelineMetrics,
}

impl Drop for PoolHold {
    fn drop(&mut self) {
        self.metrics.hold_duration.record(
            self.started.elapsed().as_secs_f64(),
            &[KeyValue::new("pool", self.pool)],
        );
        if self.pool == "run" {
            self.metrics
                .runs
                .add(1, &[KeyValue::new("event", "stopped")]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry::metrics::MeterProvider;
    use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};
    use opentelemetry_sdk::metrics::{InMemoryMetricExporter, SdkMeterProvider};

    fn setup() -> (SdkMeterProvider, InMemoryMetricExporter, PipelineMetrics) {
        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder()
            .with_periodic_exporter(exporter.clone())
            .build();
        let metrics = PipelineMetrics::new(provider.meter("pipeline-test"));
        (provider, exporter, metrics)
    }

    fn sum(exporter: &InMemoryMetricExporter, name: &str) -> Option<i64> {
        exporter
            .get_finished_metrics()
            .unwrap()
            .iter()
            .flat_map(|resource| resource.scope_metrics())
            .flat_map(|scope| scope.metrics())
            .find_map(|metric| {
                if metric.name() != name {
                    return None;
                }
                match metric.data() {
                    AggregatedMetrics::I64(MetricData::Sum(sum)) => {
                        Some(sum.data_points().map(|p| p.value()).sum())
                    }
                    AggregatedMetrics::U64(MetricData::Sum(sum)) => {
                        Some(sum.data_points().map(|p| p.value() as i64).sum())
                    }
                    _ => None,
                }
            })
    }

    #[test]
    fn disabled_pipeline_does_not_construct_instruments() {
        const CHILD: &str = "RUNTARA_TEST_DISABLED_PIPELINE_CHILD";
        if std::env::var_os(CHILD).is_some() {
            assert!(!telemetry_enabled());
            assert!(PipelineMetrics::global().is_none());
        } else {
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "pipeline_metrics::tests::disabled_pipeline_does_not_construct_instruments",
                ])
                .env(CHILD, "1")
                .env("OTEL_SDK_DISABLED", "TrUe")
                .status()
                .unwrap();
            assert!(status.success());
        }
    }

    #[test]
    fn occupancy_reads_semaphores_and_does_not_retain_owners() {
        let (provider, exporter, metrics) = setup();
        let permits = Arc::new(Semaphore::new(4));
        metrics.observe_pool("preparation", &permits, 4);
        let held = permits.clone().try_acquire_many_owned(2).unwrap();
        provider.force_flush().unwrap();
        assert_eq!(sum(&exporter, "runtara.pipeline.pool.usage"), Some(2));
        assert_eq!(sum(&exporter, "runtara.pipeline.pool.capacity"), Some(4));
        drop(held);
        exporter.reset();
        provider.force_flush().unwrap();
        assert_eq!(sum(&exporter, "runtara.pipeline.pool.usage"), Some(0));
        let weak = Arc::downgrade(&permits);
        drop(permits);
        assert!(
            weak.upgrade().is_none(),
            "callbacks must not extend owner lifetime"
        );
        exporter.reset();
        provider.force_flush().unwrap();
        assert!(sum(&exporter, "runtara.pipeline.pool.capacity").is_none_or(|n| n == 0));
        provider.shutdown().unwrap();
    }

    #[test]
    fn cancellation_or_unwind_records_one_release_and_duration() {
        let (provider, exporter, metrics) = setup();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _hold = metrics.hold("run");
            panic!("simulated task failure");
        }));
        assert!(result.is_err());
        provider.force_flush().unwrap();
        assert_eq!(sum(&exporter, "runtara.runner.runs"), Some(2));
        let snapshots = exporter.get_finished_metrics().unwrap();
        let duration = snapshots
            .iter()
            .flat_map(|r| r.scope_metrics())
            .flat_map(|s| s.metrics())
            .find(|m| m.name() == "runtara.pipeline.pool.hold.duration")
            .unwrap();
        let AggregatedMetrics::F64(MetricData::Histogram(histogram)) = duration.data() else {
            panic!("expected histogram")
        };
        assert_eq!(histogram.data_points().map(|p| p.count()).sum::<u64>(), 1);
        provider.shutdown().unwrap();
    }

    #[test]
    fn transition_and_wait_metrics_have_only_bounded_attributes() {
        let (provider, exporter, metrics) = setup();
        metrics.transition(LaunchState::Failed, "queue_timeout");
        metrics.queue_duration(2.5, "expired");
        provider.force_flush().unwrap();
        assert_eq!(sum(&exporter, "runtara.launch.transitions"), Some(1));
        for resource in exporter.get_finished_metrics().unwrap() {
            for scope in resource.scope_metrics() {
                for metric in scope.metrics() {
                    let attrs: Vec<_> = match metric.data() {
                        AggregatedMetrics::U64(MetricData::Sum(s)) => {
                            s.data_points().flat_map(|p| p.attributes()).collect()
                        }
                        AggregatedMetrics::F64(MetricData::Histogram(h)) => {
                            h.data_points().flat_map(|p| p.attributes()).collect()
                        }
                        _ => panic!("unexpected instrument"),
                    };
                    assert!(
                        attrs
                            .iter()
                            .all(|a| matches!(a.key.as_str(), "state" | "reason" | "outcome"))
                    );
                }
            }
        }
        provider.shutdown().unwrap();
    }
}
