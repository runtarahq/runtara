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
/// `RUNTARA_RUNNER` is honored only to warn — the CLI process runner
/// (`wasm` / `wasmtime`) was removed after the embedded engine became the
/// default, so every value resolves to the embedded runner.
pub fn runner_from_env(
    persistence: std::sync::Arc<dyn runtara_core::persistence::Persistence>,
) -> Result<std::sync::Arc<dyn Runner>> {
    let requested = std::env::var("RUNTARA_RUNNER").unwrap_or_default();
    match requested.to_ascii_lowercase().as_str() {
        "" | "embedded" | "wasm-embedded" => {}
        other => {
            tracing::warn!(
                requested = %other,
                "RUNTARA_RUNNER is set but the CLI process runner has been removed; \
                 using the embedded in-process engine"
            );
        }
    }
    let runner = EmbeddedWasmRunner::new(WorkflowRunnerConfig::from_env(), persistence)?;
    tracing::info!("Using EmbeddedWasmRunner (in-process wasmtime) for workflow execution");
    Ok(std::sync::Arc::new(runner))
}
