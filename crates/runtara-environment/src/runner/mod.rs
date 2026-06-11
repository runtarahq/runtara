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
/// - `embedded` / `wasm-embedded` / unset → in-process embedded wasmtime
///   engine (default)
/// - `wasm` / `wasmtime` → CLI process runner; needs a `wasmtime` binary
///   (`WASMTIME_PATH`), kept as an escape hatch
///
/// Unknown values warn and fall back to the default.
pub fn runner_from_env(
    persistence: std::sync::Arc<dyn runtara_core::persistence::Persistence>,
) -> Result<std::sync::Arc<dyn Runner>> {
    let requested = std::env::var("RUNTARA_RUNNER").unwrap_or_default();
    match requested.to_ascii_lowercase().as_str() {
        "wasm" | "wasmtime" => Ok(std::sync::Arc::new(WasmRunner::new(
            WasmRunnerConfig::from_env(),
            persistence,
        ))),
        "" | "embedded" | "wasm-embedded" => {
            let runner = EmbeddedWasmRunner::new(WasmRunnerConfig::from_env(), persistence)?;
            tracing::info!("Using EmbeddedWasmRunner (in-process wasmtime) for workflow execution");
            Ok(std::sync::Arc::new(runner))
        }
        other => {
            tracing::warn!(
                requested = %other,
                "RUNTARA_RUNNER value not recognized (expected embedded | wasm | wasmtime); \
                 using the embedded runner"
            );
            let runner = EmbeddedWasmRunner::new(WasmRunnerConfig::from_env(), persistence)?;
            Ok(std::sync::Arc::new(runner))
        }
    }
}
