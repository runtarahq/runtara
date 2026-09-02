// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Runner module - instance execution backends.
//!
//! This module is moved from runtara-core.

mod common;
pub mod embedded;
pub mod mock;
mod traits;

pub use common::WorkflowRunnerConfig;
pub use embedded::EmbeddedWasmRunner;
pub use mock::MockRunner;
pub use traits::*;

/// Build the workflow runner: the in-process embedded wasmtime engine.
///
/// `RUNTARA_RUNNER` selected between backends when more than one existed. It is
/// now accepted and ignored, with a warning, so a stale operator config does not
/// fail a boot.
pub fn build_runner(
    persistence: std::sync::Arc<dyn runtara_core::persistence::Persistence>,
    event_observer: Option<
        std::sync::Arc<dyn runtara_core::instance_handlers::InstanceEventObserver>,
    >,
) -> Result<std::sync::Arc<dyn Runner>> {
    if let Ok(requested) = std::env::var("RUNTARA_RUNNER")
        && !requested.is_empty()
    {
        tracing::warn!(
            requested = %requested,
            "RUNTARA_RUNNER is set but only the embedded in-process engine exists; ignoring"
        );
    }
    let mut runner = EmbeddedWasmRunner::new(WorkflowRunnerConfig::from_env(), persistence)?;
    if let Some(observer) = event_observer {
        runner = runner.with_event_observer(observer);
    }
    tracing::info!("Using EmbeddedWasmRunner (in-process wasmtime) for workflow execution");
    Ok(std::sync::Arc::new(runner))
}
