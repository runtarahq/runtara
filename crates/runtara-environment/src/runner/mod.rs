// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Runner module - instance execution backends.
//!
//! This module is moved from runtara-core.

mod common;
pub mod embedded;
pub mod mock;
mod traits;
pub mod wasm;

pub use embedded::EmbeddedWasmRunner;
pub use mock::MockRunner;
pub use traits::*;
pub use wasm::{WasmRunner, WasmRunnerConfig};

/// Build the workflow runner selected by `RUNTARA_RUNNER`.
///
/// - `wasm` / `wasmtime` / unset → CLI process runner (default)
/// - `embedded` / `wasm-embedded` → in-process embedded wasmtime engine
///
/// Unknown values warn and fall back to the default, preserving the
/// historical behavior of this knob.
pub fn runner_from_env(
    persistence: std::sync::Arc<dyn runtara_core::persistence::Persistence>,
) -> Result<std::sync::Arc<dyn Runner>> {
    let requested = std::env::var("RUNTARA_RUNNER").unwrap_or_default();
    match requested.to_ascii_lowercase().as_str() {
        "embedded" | "wasm-embedded" => {
            let runner = EmbeddedWasmRunner::new(WasmRunnerConfig::from_env(), persistence)?;
            tracing::info!("Using EmbeddedWasmRunner (in-process wasmtime) for workflow execution");
            Ok(std::sync::Arc::new(runner))
        }
        "" | "wasm" | "wasmtime" => Ok(std::sync::Arc::new(WasmRunner::new(
            WasmRunnerConfig::from_env(),
            persistence,
        ))),
        other => {
            tracing::warn!(
                requested = %other,
                "RUNTARA_RUNNER value not recognized (expected wasm | wasmtime | embedded); \
                 using the wasmtime CLI runner"
            );
            Ok(std::sync::Arc::new(WasmRunner::new(
                WasmRunnerConfig::from_env(),
                persistence,
            )))
        }
    }
}
