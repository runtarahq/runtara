// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Host-side surface for the `runtara:workflow/runtime` interface.
//!
//! Every composed workflow (see `runtara-workflows::direct_wasm`) lists
//! `runtara:workflow/runtime` among its component-level imports; the
//! host is its only implementation. This module provides it: a
//! [`RuntimeHost`] trait carrying the interface's guest-visible semantics, and
//! [`add_runtime_to_linker`] which binds every interface function to the trait
//! via `func_wrap_async`.
//!
//! Layering: this crate stays persistence-agnostic. The trait is DEFINED here;
//! the production implementation lives in `runtara-environment`, delegating to
//! `runtara-core::instance_handlers` over `Arc<dyn Persistence>`.
//!
//! Three interface functions are handled locally in the glue and never reach
//! the trait:
//! - `now-ms` — wall clock.
//! - `blocking-sleep` — plain (non-durable) sleep for the requested duration.
//! - `durable-sleep` — aliased to `durable-sleep-checkpoint` under
//!   [`DURABLE_SLEEP_CHECKPOINT_ID`].

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use wasmtime::StoreContextMut;
use wasmtime::component::Linker;

use crate::host_io::HostIoContext;
use crate::workflow::WorkflowState;

/// Fully-qualified component import name of the runtime interface.
///
/// Must match `runtara:workflow@1.0.0`'s `runtime` interface as
/// emitted into the workflow world by `runtara-workflows::direct_wasm`
/// (`emit_world_wit`) — the Spike-B integration test asserts a HostImport
/// composition surfaces exactly this name.
pub use runtara_wit::workflow::RUNTIME as RUNTIME_INTERFACE_NAME;

/// Checkpoint id used for plain `durable-sleep`: the host glue aliases
/// `durable-sleep` to `durable-sleep-checkpoint` under this key. The value is
/// persisted in existing checkpoints, so it must not change.
pub const DURABLE_SLEEP_CHECKPOINT_ID: &str = "__direct_workflow_runtime_durable_sleep";

/// WIT mirror of the authoritative managed-input state.
#[derive(
    Debug, Clone, PartialEq, Eq, wasmtime::component::ComponentType, wasmtime::component::Lower,
)]
#[component(variant)]
pub enum RuntimeInputState {
    /// Awaiting input before the persistence deadline.
    #[component(name = "open")]
    Open,
    /// Immutable accepted response, retained for replay.
    #[component(name = "accepted")]
    Accepted(Vec<u8>),
    /// Closed without a response; value is a machine-readable reason.
    #[component(name = "closed")]
    Closed(String),
}

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
    /// Identity of the delivered lifecycle command.
    #[component(name = "command-id")]
    pub command_id: String,
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
    /// Identity of this retained value, distinct from its checkpoint address.
    #[component(name = "signal-id")]
    pub signal_id: String,
    /// Checkpoint/wait address the retained value targets.
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
/// Semantics contract: each method follows the corresponding
/// `runtara-core::instance_handlers` handler. In particular:
///
/// - `is_cancelled`/`check_signals` acknowledge consumed lifecycle signals
///   server-side (status transitions included) through core's signal
///   acknowledgement.
/// - `durable_sleep_checkpoint` mirrors core `handle_sleep`: persist the
///   checkpoint, then sleep the FULL duration in-process (no resume-remaining
///   math — parity with today's guest-visible behavior; the suspend/re-invoke
///   model arrives in a later phase).
/// - Ordinary errors are returned as guest-visible `Err(String)` (the WIT
///   `result`'s err arm). Host misconfiguration and unconfirmed managed-input
///   abandonment trap: the guest must not recover past a failed mandatory close.
///
/// Scoped child hosts capture terminal callbacks locally and report lifecycle
/// receipts to a root-owned coordinator instead of applying root transitions.
/// The embedding finalizes those effects after invocation teardown; it must
/// supply explicit checkpoint authority and persistent attempt fencing.
#[async_trait::async_trait]
pub trait RuntimeHost: Send + Sync {
    /// Persisted input for this instance; `None` when the record has no input
    /// (the glue substitutes the `{}` envelope).
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
    /// Read a lifecycle command without acknowledging it or publishing status.
    /// May be rate-limited. The guest retains observed intent until cleanup and
    /// uses `handle_checkpoint_signal` to acknowledge the exact command later.
    async fn poll_signal(&self) -> Result<Option<RuntimeSignalInfo>, String>;
    /// True when a cancel signal is pending or was already consumed.
    async fn is_cancelled(&self) -> Result<bool, String>;
    /// Poll lifecycle signals; true when a stop-like signal was handled and
    /// the guest should return.
    async fn check_signals(&self) -> Result<bool, String>;
    /// Poll a custom signal scoped to `checkpoint_id`.
    async fn poll_custom_signal(&self, checkpoint_id: String) -> Result<Option<Vec<u8>>, String>;
    /// Register a managed wait under this host's immutable execution authority.
    async fn register_input(
        &self,
        _descriptor: Vec<u8>,
        _deadline_ms: Option<u64>,
    ) -> Result<(), String> {
        Err("runtime does not support managed inputs".into())
    }
    /// Authoritative response read and deadline arbitration.
    async fn poll_input(&self, _signal_id: String) -> Result<RuntimeInputState, String> {
        Err("runtime does not support managed inputs".into())
    }
    /// Abandon a wait under this host's authority; accepted responses are retained.
    /// The native import traps on failure so guest error handling cannot continue
    /// with an orphaned request. Supervision must settle the failed execution.
    async fn close_input(&self, _signal_id: String) -> Result<RuntimeInputState, String> {
        Err("runtime does not support managed inputs".into())
    }
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
    async fn handle_checkpoint_signal(
        &self,
        signal_type: String,
        command_id: String,
    ) -> Result<bool, String>;
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
    /// The continuation an operation-scoped call site saved for `attempt`, if
    /// any. `op_hash` is the sha256 of the site's canonical checkpoint key; the
    /// host scopes it to this instance. Hosts without typed agent suspension
    /// refuse, so a suspending step fails loudly instead of losing state.
    async fn operation_continuation_load(
        &self,
        _op_hash: String,
        _attempt: u32,
    ) -> Result<Option<Vec<u8>>, String> {
        Err("runtime does not support typed agent suspension".into())
    }
    /// Persist the continuation of a suspended operation (at most
    /// `runtara_agent_suspension::MAX_CONTINUATION_BYTES`), replacing any
    /// earlier one for the same `(op_hash, attempt)`.
    async fn operation_continuation_store(
        &self,
        _op_hash: String,
        _attempt: u32,
        _state: Vec<u8>,
    ) -> Result<(), String> {
        Err("runtime does not support typed agent suspension".into())
    }
    /// Close the operation's instance wait, if it registered one, so a retry
    /// of the operation registers afresh. Idempotent: an operation without a
    /// wait, or with a closed one, is fine.
    async fn operation_wait_close(&self, _op_hash: String) -> Result<(), String> {
        Err("runtime does not support typed agent suspension".into())
    }
    /// Drop everything kept for an operation: its continuation and its
    /// settled or closed instance wait. Idempotent.
    async fn operation_release(&self, _op_hash: String) -> Result<(), String> {
        Err("runtime does not support typed agent suspension".into())
    }
    /// Milliseconds since the UNIX epoch, as the guest sees them.
    ///
    /// Defaults to the wall clock, which is what every production host uses. It
    /// is a trait method rather than a free function so a harness standing in
    /// for the wake scheduler can advance to a park's deadline instead of
    /// waiting out real time: a resumed durable Delay compares `now-ms` against
    /// its stored deadline to decide whether to continue or re-park, so without
    /// a movable clock a park can only ever be observed re-parking.
    fn now_ms(&self) -> Result<u64, String> {
        wall_clock_now_ms()
    }

    /// How the host launched this run, which decides whether a trusted call
    /// under an earlier approved pin may run (`TrustedExecutor::admits`).
    /// Host authority, never read from the guest. The default is the
    /// strictest, `Start`; a host that relaunches parked runs overrides it,
    /// and a wrapper must delegate.
    fn trusted_launch(&self) -> crate::trusted::TrustedLaunch {
        crate::trusted::TrustedLaunch::Start
    }
}

/// Milliseconds since the UNIX epoch (the default `now-ms` implementation).
fn wall_clock_now_ms() -> Result<u64, String> {
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
    // An already selected emergency abort must also stop runtime calls between
    // epoch checks, especially terminal publication and signal acknowledgement.
    // This does not roll back a host operation that began before expiry.
    if store
        .data()
        .cleanup_alarm()
        .is_some_and(|alarm| alarm.expired())
    {
        return Err(wasmtime::Error::new(
            crate::cleanup_alarm::CleanupGraceExpired,
        ));
    }
    store.data().runtime_host().cloned().ok_or_else(|| {
        wasmtime::format_err!(
            "workflow imports {RUNTIME_INTERFACE_NAME} but the run was not configured \
             with a RuntimeHost (WorkflowRunSpec.runtime is None)"
        )
    })
}

/// Bind every `runtara:workflow/runtime` function to the store's
/// [`RuntimeHost`].
///
/// Registering these definitions is non-invasive for components that do not
/// import the interface — wasmtime only consults linker definitions for
/// imports a component actually declares (the same way the full WASI surface
/// coexists with minimal components).
pub fn add_runtime_to_linker(linker: &mut Linker<WorkflowState>) -> anyhow::Result<()> {
    let mut inst = linker.instance(RUNTIME_INTERFACE_NAME)?;
    inst.func_wrap_async(
        "poll-signal",
        |mut store: StoreContextMut<'_, WorkflowState>, (): ()| {
            let host = require_host(&mut store);
            Box::new(async move { Ok((host?.poll_signal().await,)) })
        },
    )?;

    inst.func_wrap_async(
        "load-input",
        |mut store: StoreContextMut<'_, WorkflowState>, (): ()| {
            let host = require_host(&mut store);
            Box::new(async move {
                let host = host?;
                // Absent input loads as the empty JSON envelope, never as an
                // error.
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
        "register-input",
        |mut store: StoreContextMut<'_, WorkflowState>,
         (descriptor, deadline): (Vec<u8>, Option<u64>)| {
            let host = require_host(&mut store);
            Box::new(async move { Ok((host?.register_input(descriptor, deadline).await,)) })
        },
    )?;
    inst.func_wrap_async(
        "poll-input",
        |mut store: StoreContextMut<'_, WorkflowState>, (signal_id,): (String,)| {
            let host = require_host(&mut store);
            Box::new(async move { Ok((host?.poll_input(signal_id).await,)) })
        },
    )?;
    inst.func_wrap_async(
        "close-input",
        |mut store: StoreContextMut<'_, WorkflowState>, (signal_id,): (String,)| {
            let host = require_host(&mut store);
            Box::new(async move {
                let state = host?.close_input(signal_id).await.map_err(|_| {
                    wasmtime::format_err!("managed input abandonment could not be confirmed")
                })?;
                Ok((Ok::<RuntimeInputState, String>(state),))
            })
        },
    )?;

    inst.func_wrap_async(
        "now-ms",
        |mut store: StoreContextMut<'_, WorkflowState>, (): ()| {
            let host = require_host(&mut store);
            Box::new(async move { Ok((host?.now_ms(),)) })
        },
    )?;

    inst.func_wrap_async(
        "durable-sleep",
        |mut store: StoreContextMut<'_, WorkflowState>, (ms,): (u64,)| {
            let host = require_host(&mut store);
            Box::new(async move {
                // Alias to durable-sleep-checkpoint under the fixed key.
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
                // An async sleep returns after `ms`, like a blocking sleep,
                // without pinning an executor thread.
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
        |mut store: StoreContextMut<'_, WorkflowState>,
         (signal_type, command_id): (String, String)| {
            let host = require_host(&mut store);
            Box::new(async move {
                Ok((host?
                    .handle_checkpoint_signal(signal_type, command_id)
                    .await,))
            })
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
    fn the_runtime_links_with_its_signal_observation_contract() {
        let engine = crate::build_engine(&crate::EngineConfig {
            cache_dir: None,
            enable_epoch_interruption: false,
        })
        .unwrap();
        let mut linker = Linker::<WorkflowState>::new(&engine);
        add_runtime_to_linker(&mut linker).unwrap();
        let observation = r#"
            (type $signal (record
                (field "signal-type" string)
                (field "command-id" string)
                (field "payload" (list u8))
                (field "checkpoint-id" (option string))))
            (export "signal-info" (type $exported-signal (eq $signal)))
            (export "poll-signal" (func (result (result (option $exported-signal) (error string)))))
        "#;
        for (version, poll, expected) in [
            (RUNTIME_INTERFACE_NAME, observation, true),
            (RUNTIME_INTERFACE_NAME, "", true),
        ] {
            let component = wasmtime::component::Component::new(
                &engine,
                format!(
                    r#"
                (component (import "{version}" (instance
                    {poll}
                    (export "is-cancelled" (func (result (result bool (error string)))))
                    (export "handle-checkpoint-signal" (func
                        (param "signal-type" string) (param "command-id" string)
                        (result (result bool (error string)))))
                )))
            "#
                ),
            )
            .unwrap();
            assert_eq!(
                linker.instantiate_pre(&component).is_ok(),
                expected,
                "{version}"
            );
        }
    }

    #[test]
    fn now_ms_is_epoch_scaled() {
        let value = wall_clock_now_ms().expect("clock after epoch");
        // 2020-01-01 in ms — sanity floor that catches unit mistakes
        // (seconds vs milliseconds) without pinning a wall clock.
        assert!(value > 1_577_836_800_000);
    }
}
