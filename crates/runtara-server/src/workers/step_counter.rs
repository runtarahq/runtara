// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Bridges guest step events into the pipeline counters.
//!
//! Its own module rather than a corner of `pipeline_gauges`, so that module's
//! "does anything actually write to this counter" test has a clean line to draw:
//! a definition sitting next to its own caller proves nothing about whether
//! production reaches it.

use std::sync::Arc;

use crate::workers::pipeline_gauges::PipelineGauges;

/// Feeds guest step events into the pipeline counters.
///
/// runtara-core defines [`InstanceEventObserver`] and this implements it, which
/// is how the counter reaches a crate that cannot depend on this one. Core
/// reports *every* persisted event and reads nothing into the subtype, so
/// selecting the ones that mean "a step started" happens here, where the
/// workflow vocabulary is known.
///
/// Past the subtype comparison the whole body is one relaxed atomic add, as
/// the trait requires: this runs on the event path of every event of every
/// workflow.
pub struct StepCounter {
    gauges: Arc<PipelineGauges>,
}

impl StepCounter {
    /// Wrap the gauges as an observer core can hold.
    pub fn new(gauges: Arc<PipelineGauges>) -> Arc<Self> {
        Arc::new(Self { gauges })
    }
}

impl runtara_core::instance_handlers::InstanceEventObserver for StepCounter {
    fn on_event_persisted(&self, subtype: Option<&str>) {
        // Only a step's *start* counts. Counting its end too would double the
        // reported rate, and counting log events would inflate it.
        if subtype == Some(runtara_environment::step_vocabulary::workflow_steps().start_subtype()) {
            self.gauges.record_step();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtara_core::instance_handlers::InstanceEventObserver;

    /// Core hands over every event, so the selection has to happen here — and
    /// it has to select on the vocabulary rather than on a literal that could
    /// drift away from what compiled workflows emit.
    #[test]
    fn counts_step_starts_and_nothing_else() {
        let vocabulary = runtara_environment::step_vocabulary::workflow_steps();
        let gauges = PipelineGauges::new();
        let counter = StepCounter::new(Arc::clone(&gauges));

        counter.on_event_persisted(Some(vocabulary.start_subtype()));
        assert_eq!(gauges.totals().steps, 1, "a step start must be counted");

        counter.on_event_persisted(Some(vocabulary.end_subtype()));
        counter.on_event_persisted(Some("workflow_log"));
        counter.on_event_persisted(None);
        assert_eq!(
            gauges.totals().steps,
            1,
            "ends, logs and subtype-less events must not inflate the rate"
        );
    }
}
