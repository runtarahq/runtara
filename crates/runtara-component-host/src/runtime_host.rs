// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Host-side surface for the `runtara:workflow-runtime/runtime` interface.
//!
//! A composed workflow whose `RuntimeBinding` is `HostImport` (see
//! `runtara-workflows::direct_wasm`) lists `runtara:workflow-runtime/runtime`
//! among its component-level imports instead of satisfying it internally with
//! the composed guest runtime component (which loops back to core over
//! `wasi:http`). This module provides the native replacement: a [`RuntimeHost`]
//! trait mirroring the interface's guest-visible semantics, and
//! [`add_runtime_to_linker`] which binds every interface function to the trait
//! via `func_wrap_async`.
//!
//! Layering: this crate stays persistence-agnostic. The trait is DEFINED here;
//! the production implementation lives in `runtara-environment`, delegating to
//! `runtara-core::instance_handlers` over `Arc<dyn Persistence>` (never the
//! SDK's `EmbeddedBackend`, whose per-call `block_on` would nest runtimes).
//!
//! Three interface functions are handled locally in the glue and never reach
//! the trait, mirroring the guest runtime component they replace:
//! - `now-ms` — wall clock.
//! - `blocking-sleep` — plain (non-durable) sleep for the requested duration.
//! - `durable-sleep` — aliased to `durable-sleep-checkpoint` under
//!   [`DURABLE_SLEEP_CHECKPOINT_ID`], exactly like the guest runtime does.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use wasmtime::StoreContextMut;
use wasmtime::component::Linker;

use crate::workflow::WorkflowState;

/// Fully-qualified component import name of the runtime interface.
///
/// Must match `runtara:workflow-runtime@0.1.0`'s `runtime` interface as
/// emitted into the workflow world by `runtara-workflows::direct_wasm`
/// (`emit_world_wit`) — the Spike-B integration test asserts a HostImport
/// composition surfaces exactly this name.
pub const RUNTIME_INTERFACE_NAME: &str = "runtara:workflow-runtime/runtime@0.1.0";

/// Checkpoint id the guest runtime component uses for plain `durable-sleep`
/// (see `runtara-workflow-runtime/src/lib.rs::durable_sleep`). The host glue
/// aliases `durable-sleep` to `durable-sleep-checkpoint` under this key for
/// byte-identical persistence behavior.
pub const DURABLE_SLEEP_CHECKPOINT_ID: &str = "__direct_workflow_runtime_durable_sleep";

/// WIT mirror of `runtime.signal-info`.
///
/// Field order and kebab names must match the WIT record exactly — wasmtime
/// type-checks them against the component's import at instantiation.
#[derive(
    Debug, Clone, PartialEq, Eq, wasmtime::component::ComponentType, wasmtime::component::Lower,
)]
#[component(record)]
pub struct RuntimeSignalInfo {
    /// One of "cancel" | "pause" | "resume" | "shutdown".
    #[component(name = "signal-type")]
    pub signal_type: String,
    /// Signal payload bytes.
    pub payload: Vec<u8>,
    /// Checkpoint the signal targets, when scoped.
    #[component(name = "checkpoint-id")]
    pub checkpoint_id: Option<String>,
}

/// WIT mirror of `runtime.custom-signal-info`.
#[derive(
    Debug, Clone, PartialEq, Eq, wasmtime::component::ComponentType, wasmtime::component::Lower,
)]
#[component(record)]
pub struct RuntimeCustomSignalInfo {
    /// The signal id (checkpoint id) the payload targets.
    #[component(name = "checkpoint-id")]
    pub checkpoint_id: String,
    /// Signal payload bytes.
    pub payload: Vec<u8>,
}

/// WIT mirror of `runtime.checkpoint-result`.
#[derive(
    Debug, Clone, PartialEq, Eq, wasmtime::component::ComponentType, wasmtime::component::Lower,
)]
#[component(record)]
pub struct RuntimeCheckpointResult {
    /// True when an existing checkpoint was found (resume path).
    pub found: bool,
    /// The stored state on a hit; empty on a miss.
    pub state: Vec<u8>,
    /// Pending instance-wide signal, if any.
    #[component(name = "pending-signal")]
    pub pending_signal: Option<RuntimeSignalInfo>,
    /// Pending custom signal scoped to this checkpoint id, if any.
    #[component(name = "custom-signal")]
    pub custom_signal: Option<RuntimeCustomSignalInfo>,
}

/// Native implementation surface for the runtime interface.
///
/// Semantics contract: each method must be observably equivalent to the guest
/// runtime component + HTTP SDK backend + core guest-protocol handler chain it
/// replaces (see `runtara-workflow-runtime/src/lib.rs` for the guest side and
/// `runtara-core::instance_handlers` for the server side). In particular:
///
/// - `is_cancelled`/`check_signals` acknowledge consumed lifecycle signals
///   server-side (status transitions included) exactly like the SDK's
///   `acknowledge_cancellation`/`acknowledge_pause`/`acknowledge_shutdown`.
/// - `durable_sleep_checkpoint` mirrors core `handle_sleep`: persist the
///   checkpoint, then sleep the FULL duration in-process (no resume-remaining
///   math — parity with today's guest-visible behavior; the suspend/re-invoke
///   model arrives in a later phase).
/// - Errors are returned as guest-visible `Err(String)` (the WIT `result`'s
///   err arm), not traps; a trap is reserved for host misconfiguration.
#[async_trait::async_trait]
pub trait RuntimeHost: Send + Sync {
    /// Persisted input for this instance; `None` when the record has no input
    /// (the glue substitutes the `{}` envelope, matching the guest runtime).
    async fn load_input(&self) -> Result<Option<Vec<u8>>, String>;
    /// This run's instance id.
    fn instance_id(&self) -> Result<String, String>;
    /// Report terminal success with the output payload.
    async fn complete(&self, output: Vec<u8>) -> Result<(), String>;
    /// Report terminal failure with the error payload.
    async fn fail(&self, error: Vec<u8>) -> Result<(), String>;
    /// Emit a custom event (`kind` becomes the event subtype).
    async fn custom_event(&self, kind: String, payload: Vec<u8>) -> Result<(), String>;
    /// Whether step-level debug instrumentation is enabled for this run.
    fn debug_mode_enabled(&self) -> Result<bool, String>;
    /// Suspend at a breakpoint: acknowledge a pause and mark suspended.
    async fn breakpoint_pause(&self) -> Result<(), String>;
    /// Liveness heartbeat.
    async fn heartbeat(&self) -> Result<(), String>;
    /// True when a cancel signal is pending or was already consumed.
    async fn is_cancelled(&self) -> Result<bool, String>;
    /// Poll lifecycle signals; true when a stop-like signal was handled and
    /// the guest should return.
    async fn check_signals(&self) -> Result<bool, String>;
    /// Poll a custom signal scoped to `checkpoint_id`.
    async fn poll_custom_signal(&self, checkpoint_id: String) -> Result<Option<Vec<u8>>, String>;
    /// Read-only checkpoint lookup.
    async fn get_checkpoint(&self, checkpoint_id: String) -> Result<Option<Vec<u8>>, String>;
    /// Combined save/load checkpoint (see core `handle_checkpoint`).
    async fn checkpoint(
        &self,
        checkpoint_id: String,
        state: Vec<u8>,
    ) -> Result<RuntimeCheckpointResult, String>;
    /// React to a pending signal reported by a checkpoint result; true when a
    /// stop-like signal was handled and the guest should return.
    async fn handle_checkpoint_signal(&self, signal_type: String) -> Result<bool, String>;
    /// Record a retry attempt (write-only audit trail).
    async fn record_retry_attempt(
        &self,
        checkpoint_id: String,
        attempt_number: u32,
        error_message: Option<String>,
    ) -> Result<(), String>;
    /// Persist a wake checkpoint, then sleep the full duration in-process.
    async fn durable_sleep_checkpoint(
        &self,
        checkpoint_id: String,
        state: Vec<u8>,
        ms: u64,
    ) -> Result<(), String>;
}

/// Milliseconds since the UNIX epoch (the `now-ms` implementation).
fn now_ms() -> Result<u64, String> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?;
    u64::try_from(elapsed.as_millis())
        .map_err(|_| "current UNIX timestamp does not fit in u64 milliseconds".to_string())
}

/// Clone the run's `RuntimeHost` out of the store, or trap with a diagnosis.
///
/// A component that imports the runtime interface but runs without a
/// configured host is a wiring bug (e.g. a HostImport artifact executed
/// through a spec that never set [`crate::workflow::WorkflowRunSpec::runtime`])
/// — trap loudly instead of surfacing a confusing guest-level error.
fn require_host(
    store: &mut StoreContextMut<'_, WorkflowState>,
) -> wasmtime::Result<Arc<dyn RuntimeHost>> {
    store.data().runtime_host().cloned().ok_or_else(|| {
        wasmtime::format_err!(
            "workflow imports {RUNTIME_INTERFACE_NAME} but the run was not configured \
             with a RuntimeHost (WorkflowRunSpec.runtime is None)"
        )
    })
}

/// Bind every `runtara:workflow-runtime/runtime` function to the store's
/// [`RuntimeHost`].
///
/// Registering these definitions is non-invasive for components that do not
/// import the interface — wasmtime only consults linker definitions for
/// imports a component actually declares (the same way the full WASI surface
/// coexists with minimal components). Old composed artifacts therefore run
/// unchanged through a linker that carries these bindings.
pub fn add_runtime_to_linker(linker: &mut Linker<WorkflowState>) -> anyhow::Result<()> {
    let mut inst = linker.instance(RUNTIME_INTERFACE_NAME)?;

    inst.func_wrap_async(
        "load-input",
        |mut store: StoreContextMut<'_, WorkflowState>, (): ()| {
            let host = require_host(&mut store);
            Box::new(async move {
                let host = host?;
                // Mirror the guest runtime: absent input loads as the empty
                // JSON envelope, never as an error.
                let result = host
                    .load_input()
                    .await
                    .map(|input| input.unwrap_or_else(|| b"{}".to_vec()));
                Ok((result,))
            })
        },
    )?;

    inst.func_wrap_async(
        "instance-id",
        |mut store: StoreContextMut<'_, WorkflowState>, (): ()| {
            let host = require_host(&mut store);
            Box::new(async move { Ok((host?.instance_id(),)) })
        },
    )?;

    inst.func_wrap_async(
        "complete",
        |mut store: StoreContextMut<'_, WorkflowState>, (output,): (Vec<u8>,)| {
            let host = require_host(&mut store);
            Box::new(async move { Ok((host?.complete(output).await,)) })
        },
    )?;

    inst.func_wrap_async(
        "fail",
        |mut store: StoreContextMut<'_, WorkflowState>, (error,): (Vec<u8>,)| {
            let host = require_host(&mut store);
            Box::new(async move { Ok((host?.fail(error).await,)) })
        },
    )?;

    inst.func_wrap_async(
        "custom-event",
        |mut store: StoreContextMut<'_, WorkflowState>, (kind, payload): (String, Vec<u8>)| {
            let host = require_host(&mut store);
            Box::new(async move { Ok((host?.custom_event(kind, payload).await,)) })
        },
    )?;

    inst.func_wrap_async(
        "debug-mode-enabled",
        |mut store: StoreContextMut<'_, WorkflowState>, (): ()| {
            let host = require_host(&mut store);
            Box::new(async move { Ok((host?.debug_mode_enabled(),)) })
        },
    )?;

    inst.func_wrap_async(
        "breakpoint-pause",
        |mut store: StoreContextMut<'_, WorkflowState>, (): ()| {
            let host = require_host(&mut store);
            Box::new(async move { Ok((host?.breakpoint_pause().await,)) })
        },
    )?;

    inst.func_wrap_async(
        "heartbeat",
        |mut store: StoreContextMut<'_, WorkflowState>, (): ()| {
            let host = require_host(&mut store);
            Box::new(async move { Ok((host?.heartbeat().await,)) })
        },
    )?;

    inst.func_wrap_async(
        "is-cancelled",
        |mut store: StoreContextMut<'_, WorkflowState>, (): ()| {
            let host = require_host(&mut store);
            Box::new(async move { Ok((host?.is_cancelled().await,)) })
        },
    )?;

    inst.func_wrap_async(
        "check-signals",
        |mut store: StoreContextMut<'_, WorkflowState>, (): ()| {
            let host = require_host(&mut store);
            Box::new(async move { Ok((host?.check_signals().await,)) })
        },
    )?;

    inst.func_wrap_async(
        "poll-custom-signal",
        |mut store: StoreContextMut<'_, WorkflowState>, (checkpoint_id,): (String,)| {
            let host = require_host(&mut store);
            Box::new(async move { Ok((host?.poll_custom_signal(checkpoint_id).await,)) })
        },
    )?;

    inst.func_wrap_async(
        "now-ms",
        |_store: StoreContextMut<'_, WorkflowState>, (): ()| {
            Box::new(async move { Ok((now_ms(),)) })
        },
    )?;

    inst.func_wrap_async(
        "durable-sleep",
        |mut store: StoreContextMut<'_, WorkflowState>, (ms,): (u64,)| {
            let host = require_host(&mut store);
            Box::new(async move {
                // Alias to durable-sleep-checkpoint under the fixed key, as
                // the guest runtime component does.
                let result = host?
                    .durable_sleep_checkpoint(
                        DURABLE_SLEEP_CHECKPOINT_ID.to_string(),
                        Vec::new(),
                        ms,
                    )
                    .await;
                Ok((result,))
            })
        },
    )?;

    inst.func_wrap_async(
        "blocking-sleep",
        |_store: StoreContextMut<'_, WorkflowState>, (ms,): (u64,)| {
            Box::new(async move {
                // The guest runtime blocks in std::thread::sleep; host-side an
                // async sleep is observably identical to the guest (the call
                // returns after `ms`) without pinning an executor thread.
                tokio::time::sleep(Duration::from_millis(ms)).await;
                Ok((Ok::<(), String>(()),))
            })
        },
    )?;

    inst.func_wrap_async(
        "get-checkpoint",
        |mut store: StoreContextMut<'_, WorkflowState>, (checkpoint_id,): (String,)| {
            let host = require_host(&mut store);
            Box::new(async move { Ok((host?.get_checkpoint(checkpoint_id).await,)) })
        },
    )?;

    inst.func_wrap_async(
        "checkpoint",
        |mut store: StoreContextMut<'_, WorkflowState>,
         (checkpoint_id, state): (String, Vec<u8>)| {
            let host = require_host(&mut store);
            Box::new(async move { Ok((host?.checkpoint(checkpoint_id, state).await,)) })
        },
    )?;

    inst.func_wrap_async(
        "handle-checkpoint-signal",
        |mut store: StoreContextMut<'_, WorkflowState>, (signal_type,): (String,)| {
            let host = require_host(&mut store);
            Box::new(async move { Ok((host?.handle_checkpoint_signal(signal_type).await,)) })
        },
    )?;

    inst.func_wrap_async(
        "record-retry-attempt",
        |mut store: StoreContextMut<'_, WorkflowState>,
         (checkpoint_id, attempt_number, error_message): (String, u32, Option<String>)| {
            let host = require_host(&mut store);
            Box::new(async move {
                Ok((host?
                    .record_retry_attempt(checkpoint_id, attempt_number, error_message)
                    .await,))
            })
        },
    )?;

    inst.func_wrap_async(
        "durable-sleep-checkpoint",
        |mut store: StoreContextMut<'_, WorkflowState>,
         (checkpoint_id, state, ms): (String, Vec<u8>, u64)| {
            let host = require_host(&mut store);
            Box::new(async move {
                Ok((host?
                    .durable_sleep_checkpoint(checkpoint_id, state, ms)
                    .await,))
            })
        },
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_ms_is_epoch_scaled() {
        let value = now_ms().expect("clock after epoch");
        // 2020-01-01 in ms — sanity floor that catches unit mistakes
        // (seconds vs milliseconds) without pinning a wall clock.
        assert!(value > 1_577_836_800_000);
    }
}
