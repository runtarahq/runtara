// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Runs the control agent's `runtara:control/execution` in fresh stores.
//!
//! The composed copy of the control agent inside a workflow forwards to
//! `runtara:control/executor`. The workflow linker binds that import to
//! [`ControlExecutor::invoke`], which instantiates the host-loaded control
//! bytes (never the composed copy, never tenant bytes) in a fresh restricted
//! store where `runtara:control/api` is real, and passes the caller
//! operation's continuation. The authority comes from the calling workflow's
//! store.
//!
//! Decision D2: the host checks the composed bytes at load and approval on
//! every call. A workflow may forward only through a [`ControlBinding`] — its
//! one `runtara:builtin-artifacts/control-…` pin plus the sha256 of every
//! composed component that imports `runtara:control/`, audited from the
//! source bytes by the precompile worker — and every call re-checks the
//! binding and the executor's own bytes against the approved history the
//! embedding installed with [`ControlExecutor::set_approved_pins`].

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;
use wasmtime::component::{Component, InstancePre, Linker};
use wasmtime::{Engine, Store, UpdateDeadline};

use crate::control_host::{ControlApiCall, ControlAuthority, ControlHost, control_error_info};
use crate::host_state::HostState;
use crate::operation_scope_host::SuspendableOutcome;

const MAX_INPUT: usize = runtara_control_contract::MAX_INPUT_BYTES;
const MAX_OUTPUT: usize = runtara_control_contract::MAX_OUTCOME_BYTES;
const MAX_STATE: usize = runtara_agent_suspension::MAX_CONTINUATION_BYTES;
const MEMORY_LIMIT: usize = 64 * 1024 * 1024;
const TIME_LIMIT: Duration =
    Duration::from_millis(runtara_control_contract::EXECUTION_TIME_LIMIT_MS);

/// Bundle file names of the control agent.
pub const CONTROL_WASM_FILENAME: &str = "runtara_agent_control.wasm";
/// Sidecar of [`CONTROL_WASM_FILENAME`].
pub const CONTROL_META_FILENAME: &str = "runtara_agent_control.meta.json";

/// What a prepared workflow artifact may forward control calls with: its
/// content pin and the audited composed importers of `runtara:control/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlBinding {
    /// The root's `runtara:builtin-artifacts/control-…` import.
    pub pin: String,
    /// Hex sha256 of each composed component importing `runtara:control/`.
    pub importers: BTreeSet<String>,
}

/// Logs routing identity and outcome only, never inputs or outputs.
struct InvocationAudit<'a> {
    tenant: &'a str,
    caller: Option<&'a str>,
    capability: &'a str,
    started: std::time::Instant,
    outcome: &'static str,
}

impl Drop for InvocationAudit<'_> {
    fn drop(&mut self) {
        tracing::info!(
            tenant = self.tenant,
            caller = self.caller,
            capability = self.capability,
            duration_ms = self.started.elapsed().as_millis() as u64,
            outcome = self.outcome,
            "control capability completed"
        );
    }
}

/// Host executor for the approved control agent.
pub struct ControlExecutor {
    engine: Arc<Engine>,
    pre: InstancePre<HostState>,
    digest: String,
    pin: String,
    host: OnceLock<Arc<dyn ControlHost>>,
    permits: Arc<Semaphore>,
    /// Approved `runtara:builtin-artifacts/control-…` pins. Empty until the
    /// embedding installs its approved history, so every call is `denied`.
    approved: RwLock<Arc<BTreeSet<String>>>,
}

impl ControlExecutor {
    /// Load `runtara_agent_control.{wasm,meta.json}` from an installed
    /// component bundle, or `None` when the bundle has no control agent.
    pub fn from_bundle(engine: Arc<Engine>, dir: &Path) -> Result<Option<Self>> {
        let wasm_path = dir.join(CONTROL_WASM_FILENAME);
        if !wasm_path.exists() {
            return Ok(None);
        }
        let wasm =
            std::fs::read(&wasm_path).with_context(|| format!("read {}", wasm_path.display()))?;
        let meta_path = dir.join(CONTROL_META_FILENAME);
        let meta =
            std::fs::read(&meta_path).with_context(|| format!("read {}", meta_path.display()))?;
        Self::new(engine, &wasm, &meta).map(Some)
    }

    /// Load the control agent from exactly the bytes the embedding approved.
    /// `meta` is its sidecar; both are part of the digest.
    pub fn new(engine: Arc<Engine>, wasm: &[u8], meta: &[u8]) -> Result<Self> {
        let component = Component::new(&engine, wasm)?;
        let ty = component.component_type();
        ty.get_export(
            &engine,
            runtara_workflow_wit::CONTROL_EXECUTION_INTERFACE_NAME,
        )
        .context("the control agent lacks the runtara:control/execution export")?;
        let pre = control_linker(&engine)?.instantiate_pre(&component)?;
        let mut hash = Sha256::new();
        hash.update(wasm);
        hash.update(meta);
        Ok(Self {
            engine,
            pre,
            digest: format!("{:x}", hash.finalize()),
            pin: control_pin(wasm, meta),
            host: OnceLock::new(),
            permits: Arc::new(Semaphore::new(16)),
            approved: RwLock::new(Arc::new(BTreeSet::new())),
        })
    }

    /// sha256 over the loaded wasm and sidecar bytes.
    pub fn digest(&self) -> &str {
        &self.digest
    }

    /// The `runtara:builtin-artifacts/control-…` pin of the loaded bytes.
    pub fn pin(&self) -> &str {
        &self.pin
    }

    /// Replace the approved history (non-revoked pins). The embedding loads
    /// it from durable storage at boot, before any run can wake.
    pub fn set_approved_pins(&self, pins: impl IntoIterator<Item = String>) {
        let pins = Arc::new(pins.into_iter().collect());
        *self
            .approved
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = pins;
    }

    /// The approved history currently in force.
    pub fn approved_pins(&self) -> Arc<BTreeSet<String>> {
        Arc::clone(
            &self
                .approved
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    /// Whether `binding` may forward control calls: its pin is approved, and
    /// every audited importer is the component of an approved pin.
    pub fn check_binding(&self, binding: &ControlBinding) -> Result<(), String> {
        let approved = self.approved_pins();
        if !approved.contains(&binding.pin) {
            return Err("the workflow's control artifact is not approved".into());
        }
        if binding.importers.is_empty() {
            return Err("no audited component imports control".into());
        }
        let components: BTreeSet<&str> = approved
            .iter()
            .filter_map(|pin| runtara_dsl::agent_meta::parse_builtin_artifact_import(pin))
            .map(|(_, wasm)| wasm)
            .collect();
        if let Some(foreign) = binding
            .importers
            .iter()
            .find(|digest| !components.contains(digest.as_str()))
        {
            return Err(format!(
                "composed component {foreign} imports control but is not an approved control agent"
            ));
        }
        Ok(())
    }

    /// Configure the native control service. Until then every call is
    /// `CONTROL_UNAVAILABLE`.
    pub fn set_host(&self, host: Arc<dyn ControlHost>) -> Result<()> {
        self.host
            .set(host)
            .map_err(|_| anyhow::anyhow!("control host already configured"))
    }

    /// Run `execution.invoke(capability, input, continuation)` in a fresh
    /// store, bounded by `deadline` and 90 s.
    // The error is the guest-visible WIT `error-info`, returned unchanged.
    #[allow(clippy::result_large_err)]
    pub async fn invoke(
        &self,
        authority: ControlAuthority,
        capability: &str,
        input: Vec<u8>,
        continuation: Option<Vec<u8>>,
        deadline: tokio::time::Instant,
    ) -> Result<SuspendableOutcome, crate::ErrorInfo> {
        let tenant = authority.tenant.clone();
        let caller = authority.caller.clone();
        let mut audit = InvocationAudit {
            tenant: &tenant,
            caller: caller.as_deref(),
            capability,
            started: std::time::Instant::now(),
            outcome: "denied",
        };
        // Decision D2: the executor's own bytes are re-checked on every call,
        // so a revoked version stops running without a restart of the store.
        if !self.approved_pins().contains(&self.pin) {
            return Err(crate::control_host::denied_error_info(
                "the installed control agent is not approved",
            ));
        }
        if authority.tenant.is_empty() {
            return Err(crate::control_host::denied_error_info(
                "control calls need a tenant",
            ));
        }
        if input.len() > MAX_INPUT {
            audit.outcome = "failure";
            return Err(control_error_info(
                "CONTROL_TOO_LARGE",
                "control input exceeds 1 MiB",
            ));
        }
        let host = self.host.get().cloned().ok_or_else(|| {
            control_error_info(
                "CONTROL_UNAVAILABLE",
                "the control service is not configured",
            )
        })?;
        audit.outcome = "cancelled";
        let deadline = deadline.min(tokio::time::Instant::now() + TIME_LIMIT);
        let timeout = || control_error_info("CONTROL_TIMEOUT", "control execution timed out");
        let mut tasks = tokio::task::JoinSet::new();
        let result = tokio::time::timeout_at(deadline, async {
            let permit = Arc::clone(&self.permits)
                .acquire_owned()
                .await
                .map_err(|_| control_error_info("CONTROL_UNAVAILABLE", "executor closed"))?;
            let engine = Arc::clone(&self.engine);
            let pre = self.pre.clone();
            let capability = capability.to_owned();
            // Wasmtime forbids nested concurrent event loops, even across
            // stores; a separate task gives this store its own. Dropping the
            // JoinSet aborts it with the caller.
            tasks.spawn(async move {
                let _permit = permit;
                Self::execute(
                    engine,
                    pre,
                    ControlApiCall { host, authority },
                    &capability,
                    input,
                    continuation,
                    deadline,
                )
                .await
            });
            tasks
                .join_next()
                .await
                .expect("one control task")
                .unwrap_or_else(|_| {
                    Err(control_error_info(
                        "CONTROL_EXECUTION_FAILED",
                        "control execution failed",
                    ))
                })
        })
        .await
        .unwrap_or_else(|_| Err(timeout()));
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
        audit.outcome = if result.is_ok() { "success" } else { "failure" };
        result
    }

    async fn execute(
        engine: Arc<Engine>,
        pre: InstancePre<HostState>,
        call: ControlApiCall,
        capability: &str,
        input: Vec<u8>,
        continuation: Option<Vec<u8>>,
        deadline: tokio::time::Instant,
    ) -> Result<SuspendableOutcome, crate::ErrorInfo> {
        let mut state = HostState::restricted();
        state.set_limits(MEMORY_LIMIT, 100_000);
        state.control_api = Some(call);
        let mut store = Store::new(&engine, state);
        store.limiter(|s| &mut s.limiter);
        store.epoch_deadline_callback(move |_| {
            Ok(if tokio::time::Instant::now() >= deadline {
                UpdateDeadline::Interrupt
            } else {
                UpdateDeadline::Yield(1)
            })
        });
        store.set_epoch_deadline(1);
        let run = async {
            let instance = pre.instantiate_async(&mut store).await?;
            let interface = instance
                .get_export_index(
                    &mut store,
                    None,
                    runtara_workflow_wit::CONTROL_EXECUTION_INTERFACE_NAME,
                )
                .context("control execution export")?;
            let export = instance
                .get_export_index(&mut store, Some(&interface), "invoke")
                .context("control execution invoke")?;
            let func = instance.get_typed_func::<
                (&str, &[u8], Option<&[u8]>),
                (Result<SuspendableOutcome, crate::ErrorInfo>,),
            >(&mut store, export)?;
            let (result,) = func
                .call_async(&mut store, (capability, &input, continuation.as_deref()))
                .await?;
            Ok::<_, anyhow::Error>(result)
        };
        let result = run.await;
        drop(store);
        match result {
            Ok(Ok(outcome)) => {
                let within = match &outcome {
                    SuspendableOutcome::Completed(output) => output.len() <= MAX_OUTPUT,
                    SuspendableOutcome::Suspended(suspension) => {
                        suspension.state.len() <= MAX_STATE
                    }
                };
                if !within {
                    return Err(control_error_info(
                        "CONTROL_TOO_LARGE",
                        "control output or continuation exceeds its limit",
                    ));
                }
                Ok(outcome)
            }
            Ok(Err(error)) => Err(error),
            Err(_) => Err(control_error_info(
                "CONTROL_EXECUTION_FAILED",
                "control execution failed or exceeded its limits",
            )),
        }
    }
}

/// The agent linker, with `runtara:control/api` real and the executor
/// `denied` (a control call never nests another).
fn control_linker(engine: &Engine) -> Result<Linker<HostState>> {
    let mut linker = crate::registry::build_base_linker(engine)?;
    crate::control_host::add_control_api_to_linker(&mut linker)?;
    crate::control_host::add_denied_control_executor_to_linker(&mut linker)?;
    Ok(linker)
}

/// The `runtara:builtin-artifacts/control-…` pin of control bytes.
pub fn control_pin(wasm: &[u8], meta: &[u8]) -> String {
    runtara_dsl::agent_meta::builtin_artifact_import(
        runtara_dsl::agent_meta::CONTROL_AGENT_ID,
        &format!("{:x}", Sha256::digest(wasm)),
        &format!("{:x}", Sha256::digest(meta)),
    )
}

/// What a workflow store knows for a forwarded control call.
#[derive(Clone)]
pub(crate) struct ControlCall {
    pub(crate) executor: Arc<ControlExecutor>,
    pub(crate) tenant: String,
    pub(crate) caller: Option<String>,
    /// The prepared artifact's audited binding; `None` (not prepared with
    /// the control audit) is always `denied`.
    pub(crate) binding: Option<Arc<ControlBinding>>,
}

/// Forward one call for a workflow store: re-check the binding (decision
/// D2), then run it with the store's authority.
async fn forward(
    call: ControlCall,
    operation: Option<String>,
    continuation: Option<Vec<u8>>,
    deadline: tokio::time::Instant,
    capability: String,
    input: Vec<u8>,
) -> Result<SuspendableOutcome, crate::ErrorInfo> {
    let Some(binding) = call.binding.as_ref() else {
        return Err(crate::control_host::denied_error_info(
            "this workflow artifact was not prepared with a control audit",
        ));
    };
    call.executor
        .check_binding(binding)
        .map_err(|reason| crate::control_host::denied_error_info(&reason))?;
    call.executor
        .invoke(
            ControlAuthority {
                tenant: call.tenant,
                caller: call.caller,
                operation,
            },
            &capability,
            input,
            continuation,
            deadline,
        )
        .await
}

/// Bind `runtara:control/executor` for workflow stores: forward to the host
/// executor with the store's authority and the entered operation's
/// continuation.
pub(crate) fn add_control_executor_to_linker(
    linker: &mut Linker<crate::workflow::WorkflowState>,
) -> Result<()> {
    linker
        .instance(runtara_workflow_wit::CONTROL_EXECUTOR_INTERFACE_NAME)?
        .func_wrap_concurrent(
            "invoke",
            |accessor, (capability, input): (String, Vec<u8>)| {
                let prepared = accessor.with(|mut access| {
                    let state = access.get();
                    let operation = state.operation.current().cloned();
                    let deadline = state.database_deadline();
                    state.control_executor.clone().map(|call| {
                        (
                            call,
                            operation.as_ref().map(|op| op.identity.op_hash.clone()),
                            operation.and_then(|op| op.continuation),
                            deadline,
                        )
                    })
                });
                Box::pin(async move {
                    let result = match prepared {
                        Some((call, operation, continuation, deadline)) => {
                            forward(call, operation, continuation, deadline, capability, input)
                                .await
                        }
                        None => Err(crate::control_host::denied_error_info(
                            "no control executor is configured for this run",
                        )),
                    };
                    // The host records the waits the approved executor
                    // returned, so a park attaches exactly those.
                    if let Ok(SuspendableOutcome::Suspended(suspension)) = &result {
                        accessor.with(|mut access| {
                            access.get().operation.record_waits(&suspension.wakes);
                        });
                    }
                    Ok((result,))
                })
            },
        )?;
    Ok(())
}
