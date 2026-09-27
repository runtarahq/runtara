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
//! store. Approval of the loaded bytes (decision D2) is enforced by the
//! embedding before it constructs the executor.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;
use wasmtime::component::{Component, InstancePre, Linker};
use wasmtime::{Engine, Store, UpdateDeadline};

use crate::control_host::{ControlApiCall, ControlAuthority, ControlHost, control_error_info};
use crate::host_state::HostState;
use crate::operation_scope_host::SuspendableOutcome;

const MAX_INPUT: usize = 1024 * 1024;
const MAX_OUTPUT: usize = 4 * 1024 * 1024;
const MEMORY_LIMIT: usize = 64 * 1024 * 1024;
const TIME_LIMIT: Duration = Duration::from_secs(90);

/// Host executor for the approved control agent.
pub struct ControlExecutor {
    engine: Arc<Engine>,
    pre: InstancePre<HostState>,
    digest: String,
    host: OnceLock<Arc<dyn ControlHost>>,
    permits: Arc<Semaphore>,
}

impl ControlExecutor {
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
            host: OnceLock::new(),
            permits: Arc::new(Semaphore::new(16)),
        })
    }

    /// sha256 over the loaded wasm and sidecar bytes.
    pub fn digest(&self) -> &str {
        &self.digest
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
        if input.len() > MAX_INPUT {
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
                let size = match &outcome {
                    SuspendableOutcome::Completed(output) => output.len(),
                    SuspendableOutcome::Suspended(suspension) => suspension.state.len(),
                };
                if size > MAX_OUTPUT {
                    return Err(control_error_info(
                        "CONTROL_TOO_LARGE",
                        "control output exceeds its limit",
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

/// What a workflow store knows for a forwarded control call.
#[derive(Clone)]
pub(crate) struct ControlCall {
    pub(crate) executor: Arc<ControlExecutor>,
    pub(crate) tenant: String,
    pub(crate) caller: Option<String>,
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
                    state.control.clone().map(|call| {
                        (
                            call,
                            operation.as_ref().map(|op| op.op_hash.clone()),
                            operation.and_then(|op| op.continuation),
                            deadline,
                        )
                    })
                });
                Box::pin(async move {
                    let result = match prepared {
                        Some((call, operation, continuation, deadline)) => {
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
                        None => Err(crate::control_host::denied_error_info(
                            "no control executor is configured for this run",
                        )),
                    };
                    Ok((result,))
                })
            },
        )?;
    Ok(())
}
