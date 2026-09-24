// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Records tracked step starts only after core persisted their event.
use opentelemetry::metrics::{Counter, Meter};
use std::sync::Arc;

pub struct StepCounter {
    starts: Counter<u64>,
}

impl StepCounter {
    pub fn new(meter: Meter) -> Arc<Self> {
        Arc::new(Self {
            starts: meter
                .u64_counter("runtara.workflow.steps.started")
                .with_description(
                    "Persisted tracked step starts; untracked workflows do not report steps",
                )
                .build(),
        })
    }
}

impl runtara_core::instance_handlers::InstanceEventObserver for StepCounter {
    fn on_event_persisted(&self, subtype: Option<&str>) {
        if subtype == Some(runtara_environment::step_vocabulary::workflow_steps().start_subtype()) {
            self.starts.add(1, &[]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry::metrics::MeterProvider;
    use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};
    use opentelemetry_sdk::metrics::{InMemoryMetricExporter, SdkMeterProvider};
    use runtara_core::instance_handlers::InstanceEventObserver;

    #[test]
    fn only_persisted_step_starts_are_counted() {
        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder()
            .with_periodic_exporter(exporter.clone())
            .build();
        let counter = StepCounter::new(provider.meter("test"));
        let vocabulary = runtara_environment::step_vocabulary::workflow_steps();
        for subtype in [
            Some(vocabulary.start_subtype()),
            Some(vocabulary.end_subtype()),
            Some("workflow_log"),
            None,
        ] {
            counter.on_event_persisted(subtype);
        }
        provider.force_flush().unwrap();
        let snapshots = exporter.get_finished_metrics().unwrap();
        let metric = snapshots
            .iter()
            .flat_map(|r| r.scope_metrics())
            .flat_map(|s| s.metrics())
            .find(|m| m.name() == "runtara.workflow.steps.started")
            .unwrap();
        let AggregatedMetrics::U64(MetricData::Sum(sum)) = metric.data() else {
            panic!("expected sum")
        };
        assert_eq!(sum.data_points().map(|p| p.value()).sum::<u64>(), 1);
        provider.shutdown().unwrap();
    }
}
