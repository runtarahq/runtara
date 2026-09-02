// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Wire types for the execution-pipeline view.
//!
//! Two nullability rules carry the honesty of this whole payload, and both cost
//! nothing to honour and a great deal to get wrong:
//!
//! - `used: None` means "this source could not be read", which is a different
//!   fact from `Some(0)`, "this stage is empty". Collapsing them renders an
//!   unobserved subsystem as an idle one.
//! - `steps: None` means "nothing live could have reported a step", which is a
//!   different fact from `Some(0.0)`, "work is live and none of it is
//!   progressing". Only the second is a symptom, and a rule that confuses them
//!   raises an alarm on a perfectly healthy deployment.
//!
//! Deliberately absent: any `health`, `severity` or `chokepoint` field. This
//! payload reports facts; classifying them is the consumer's job, where it can
//! be tested against fixtures without standing up a server.

use chrono::{DateTime, Utc};
use serde::Serialize;
use utoipa::ToSchema;

/// Throughput between pipeline stages, per second.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PipelineRatesDto {
    /// Executions presented to the admission gate.
    pub offered: f64,
    /// Executions the gate admitted.
    pub accepted: f64,
    /// Executions the gate refused with `ENTITLEMENT_LIMIT_EXCEEDED`.
    pub denied: f64,
    /// Instances handed to the runtime for launch.
    pub started: f64,
    /// Guests that stopped running, including those that parked themselves.
    pub finished: f64,
    /// Workflow steps, or `null` when nothing live could report one.
    ///
    /// `trackEvents` is compile-time, so a workflow built without it runs
    /// perfectly and emits no steps. Rendering that as zero would let a
    /// consumer declare a healthy system stalled.
    pub steps: Option<f64>,
}

/// One stage of the pipeline at one instant.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PipelineStageDto {
    /// Stable identifier: `admission`, `triggerQueue`, `triggerWorkers`,
    /// `runPermits`, `executing`, `parked`.
    pub key: String,
    /// Human-readable stage name.
    pub label: String,
    /// The setting that bounds this stage, shown verbatim so an operator can
    /// act on it without looking it up.
    pub knob: Option<String>,
    /// The bound, or `null` for a stage with no ceiling.
    pub limit: Option<u64>,
    /// Current occupancy, or `null` when the source could not be read.
    pub used: Option<u64>,
    /// Age of the oldest item held here.
    ///
    /// The signal that separates a stage turning work over from one holding
    /// work that never leaves — the two are indistinguishable by occupancy.
    pub oldest_age_ms: Option<u64>,
    /// Which rate feeds this stage, naming a field of [`PipelineRatesDto`].
    pub inflow_key: String,
}

/// The pipeline at one instant.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PipelineSnapshotDto {
    /// When this was sampled.
    pub captured_at: DateTime<Utc>,
    /// The window the rates were measured over.
    ///
    /// On the wire rather than assumed, so a consumer can tell a normal tick
    /// from one that followed a pause and discard the gap instead of drawing
    /// a spike that never happened.
    pub window_ms: u64,
    /// Throughput, or `null` on the first tick after start.
    ///
    /// There is no earlier reading to difference against then, and treating the
    /// baseline as zero would publish the process's whole lifetime of work as
    /// one second's throughput.
    pub rates: Option<PipelineRatesDto>,
    /// Every stage, in pipeline order.
    pub stages: Vec<PipelineStageDto>,
}

/// Response envelope for the pipeline endpoint.
#[derive(Debug, Serialize, ToSchema)]
pub struct PipelineSnapshotResponse {
    /// Whether the snapshot was produced.
    pub success: bool,
    /// Human-readable status.
    pub message: String,
    /// The snapshot.
    pub data: PipelineSnapshotDto,
}
