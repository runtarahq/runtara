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
/// is how the counter reaches a crate that cannot depend on this one. The whole
/// body is one relaxed atomic add, as the trait requires: this runs on the
/// event path of every step of every workflow.
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
    fn on_step_started(&self) {
        self.gauges.record_step();
    }
}
