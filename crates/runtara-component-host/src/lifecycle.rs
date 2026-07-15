// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Host-side mirror of `runtara:workflow-lifecycle/lifecycle` — the unified
//! invoke export a workflow compiled with the invoke ABI exposes instead of
//! `wasi:cli/run` (Phase 3 of docs/unify-agents-workflows-plan.md).
//!
//! Field order and kebab names must match the WIT exactly; wasmtime
//! type-checks them against the component's export when the typed function is
//! looked up.

/// Fully-qualified component export name of the lifecycle interface.
pub const LIFECYCLE_INTERFACE_NAME: &str = "runtara:workflow-lifecycle/lifecycle@0.1.0";

/// WIT mirror of `lifecycle.error-info` (field-for-field the agent error).
#[derive(
    Debug, Clone, PartialEq, Eq, wasmtime::component::ComponentType, wasmtime::component::Lift,
)]
#[component(record)]
pub struct WorkflowErrorInfo {
    pub code: String,
    pub message: String,
    pub category: String,
    pub severity: String,
    pub retryable: bool,
    #[component(name = "retry-after-ms")]
    pub retry_after_ms: Option<u64>,
    pub attributes: Option<String>,
}

/// WIT mirror of `lifecycle.signal-wait`.
#[derive(
    Debug, Clone, PartialEq, Eq, wasmtime::component::ComponentType, wasmtime::component::Lift,
)]
#[component(record)]
pub struct SignalWait {
    #[component(name = "checkpoint-id")]
    pub checkpoint_id: String,
    #[component(name = "deadline-ms")]
    pub deadline_ms: Option<u64>,
}

/// WIT mirror of `lifecycle.wake`.
#[derive(
    Debug, Clone, PartialEq, Eq, wasmtime::component::ComponentType, wasmtime::component::Lift,
)]
#[component(variant)]
pub enum WorkflowWake {
    /// Re-invoke at (or after) this wall-clock ms-since-epoch.
    #[component(name = "at")]
    At(u64),
    /// Re-invoke when the signal arrives, or at its deadline.
    #[component(name = "on-signal")]
    OnSignal(SignalWait),
    /// Lifecycle pause/drain: re-invoke on relaunch.
    #[component(name = "on-resume")]
    OnResume,
}

/// WIT mirror of `lifecycle.outcome` — the invoke success arm. `suspended`
/// carries a wake-SET (re-invoke on ANY; sequential lowering emits
/// singletons).
#[derive(
    Debug, Clone, PartialEq, Eq, wasmtime::component::ComponentType, wasmtime::component::Lift,
)]
#[component(variant)]
pub enum WorkflowOutcome {
    #[component(name = "completed")]
    Completed(Vec<u8>),
    #[component(name = "suspended")]
    Suspended(Vec<WorkflowWake>),
}
