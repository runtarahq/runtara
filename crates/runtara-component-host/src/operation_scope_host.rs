// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Host side of typed agent suspension.
//!
//! - `runtara:workflow-operation/scope`, imported only by compiled workflow
//!   logic, names the operation of a suspending call site. The host keys it by
//!   `op_hash = sha256(checkpoint-key)` inside the run's own instance; nothing
//!   about the identity comes from agent input.
//! - `runtara:agent-suspension/context.continuation()` hands an ordinary
//!   suspending agent the continuation of the entered operation. The control
//!   agent receives the same continuation as an argument of the host-called
//!   `runtara:control/execution` instead (see `control_executor`).
//!
//! Persistence goes through [`crate::runtime_host::RuntimeHost`]; hosts
//! without typed suspension refuse, which fails the step instead of losing
//! state.

use sha2::{Digest, Sha256};
use wasmtime::StoreContextMut;
use wasmtime::component::Linker;

use crate::workflow::WorkflowState;

/// WIT mirror of `runtara:agent-suspension/types.wake`.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    wasmtime::component::ComponentType,
    wasmtime::component::Lift,
    wasmtime::component::Lower,
)]
#[component(variant)]
pub enum SuspensionWake {
    /// Re-invoke at (or after) this wall-clock ms since the Unix epoch.
    #[component(name = "at")]
    At(u64),
    /// Re-invoke when this host-owned instance wait settles.
    #[component(name = "instances")]
    Instances(String),
}

/// WIT mirror of `runtara:agent-suspension/types.suspension`.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    wasmtime::component::ComponentType,
    wasmtime::component::Lift,
    wasmtime::component::Lower,
)]
#[component(record)]
pub struct Suspension {
    pub wakes: Vec<SuspensionWake>,
    pub state: Vec<u8>,
}

/// WIT mirror of `runtara:agent-suspension/types.outcome`.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    wasmtime::component::ComponentType,
    wasmtime::component::Lift,
    wasmtime::component::Lower,
)]
#[component(variant)]
pub enum SuspendableOutcome {
    #[component(name = "completed")]
    Completed(Vec<u8>),
    #[component(name = "suspended")]
    Suspended(Suspension),
}

impl SuspensionWake {
    fn to_contract(&self) -> runtara_agent_suspension::Wake {
        match self {
            Self::At(at) => runtara_agent_suspension::Wake::At(*at),
            Self::Instances(id) => runtara_agent_suspension::Wake::Instances(id.clone()),
        }
    }
}

/// The operation a call site entered and has not left.
#[derive(Debug, Clone)]
pub(crate) struct EnteredOperation {
    pub(crate) op_hash: String,
    pub(crate) attempt: u32,
    pub(crate) continuation: Option<Vec<u8>>,
}

/// Per-run operation-scope state, owned by the workflow store.
#[derive(Debug, Default)]
pub(crate) struct OperationScopeState {
    current: Option<EnteredOperation>,
    /// Instance waits attached by suspensions in this run. The host that
    /// parks the instance on them lands with durable instance waits.
    registered_waits: Vec<String>,
}

impl OperationScopeState {
    /// The entered operation, if any.
    pub(crate) fn current(&self) -> Option<&EnteredOperation> {
        self.current.as_ref()
    }

    /// Instance-wait ids the run's suspensions attached.
    pub(crate) fn registered_waits(&self) -> &[String] {
        &self.registered_waits
    }
}

/// `op_hash` of a call site: hex sha256 of its canonical checkpoint key.
pub fn operation_hash(checkpoint_key: &str) -> String {
    format!("{:x}", Sha256::digest(checkpoint_key.as_bytes()))
}

/// Store data that can answer `runtara:agent-suspension/context`.
pub(crate) trait SuspensionContextView {
    /// The continuation of the entered operation, if any.
    fn continuation(&self) -> Option<Vec<u8>>;
}

impl SuspensionContextView for WorkflowState {
    fn continuation(&self) -> Option<Vec<u8>> {
        self.operation
            .current()
            .and_then(|operation| operation.continuation.clone())
    }
}

/// Stores outside a workflow run (dispatcher, trusted and control executor
/// stores) have no operation, so no continuation.
impl SuspensionContextView for crate::host_state::HostState {
    fn continuation(&self) -> Option<Vec<u8>> {
        None
    }
}

/// Bind `runtara:agent-suspension/context`.
pub(crate) fn add_suspension_context_to_linker<T: SuspensionContextView + Send + 'static>(
    linker: &mut Linker<T>,
) -> anyhow::Result<()> {
    linker
        .instance(runtara_agent_suspension::CONTEXT_INTERFACE)?
        .func_wrap("continuation", |store: StoreContextMut<'_, T>, (): ()| {
            Ok((store.data().continuation(),))
        })?;
    Ok(())
}

fn runtime(
    store: &StoreContextMut<'_, WorkflowState>,
) -> Result<std::sync::Arc<dyn crate::runtime_host::RuntimeHost>, String> {
    store.data().runtime_host().cloned().ok_or_else(|| {
        "typed agent suspension needs a runtime host (WorkflowRunSpec.runtime is None)".to_string()
    })
}

/// Bind `runtara:workflow-operation/scope` for workflow stores.
pub(crate) fn add_operation_scope_to_linker(
    linker: &mut Linker<WorkflowState>,
) -> anyhow::Result<()> {
    let mut scope = linker.instance(runtara_workflow_wit::OPERATION_SCOPE_INTERFACE_NAME)?;
    scope.func_wrap_async(
        "enter",
        |mut store: StoreContextMut<'_, WorkflowState>,
         (key, attempt, load): (String, u32, bool)| {
            Box::new(async move {
                if store.data().operation.current.is_some() {
                    return Ok((Err(
                        "an operation is already entered; operation-scoped sites run one at a time"
                            .to_string(),
                    ),));
                }
                let op_hash = operation_hash(&key);
                let continuation = if load {
                    let host = match runtime(&store) {
                        Ok(host) => host,
                        Err(error) => return Ok((Err(error),)),
                    };
                    match host
                        .operation_continuation_load(op_hash.clone(), attempt)
                        .await
                    {
                        Ok(continuation) => continuation,
                        Err(error) => return Ok((Err(error),)),
                    }
                } else {
                    None
                };
                store.data_mut().operation.current = Some(EnteredOperation {
                    op_hash,
                    attempt,
                    continuation,
                });
                Ok((Ok(()),))
            })
        },
    )?;
    scope.func_wrap_async(
        "suspend",
        |mut store: StoreContextMut<'_, WorkflowState>,
         (state, wakes): (Vec<u8>, Vec<SuspensionWake>)| {
            Box::new(async move {
                let Some(operation) = store.data().operation.current.clone() else {
                    return Ok((Err("no operation is entered".to_string()),));
                };
                let contract: Vec<_> = wakes.iter().map(SuspensionWake::to_contract).collect();
                if let Err(error) = runtara_agent_suspension::validate_suspension(&contract, &state)
                {
                    return Ok((Err(format!(
                        "{}: {error}",
                        runtara_agent_suspension::AGENT_INVALID_SUSPENSION
                    )),));
                }
                let host = match runtime(&store) {
                    Ok(host) => host,
                    Err(error) => return Ok((Err(error),)),
                };
                if let Err(error) = host
                    .operation_continuation_store(operation.op_hash, operation.attempt, state)
                    .await
                {
                    return Ok((Err(error),));
                }
                let data = store.data_mut();
                data.operation.current = None;
                data.operation
                    .registered_waits
                    .extend(wakes.into_iter().filter_map(|wake| match wake {
                        SuspensionWake::Instances(id) => Some(id),
                        SuspensionWake::At(_) => None,
                    }));
                Ok((Ok(()),))
            })
        },
    )?;
    scope.func_wrap_async(
        "exit",
        |mut store: StoreContextMut<'_, WorkflowState>, (failed,): (bool,)| {
            Box::new(async move {
                let operation = store.data_mut().operation.current.take();
                // A failure is not checkpointed, so a replay starts the
                // operation afresh: its continuation must go with it.
                if failed && let Some(operation) = operation {
                    runtime(&store)
                        .map_err(wasmtime::Error::msg)?
                        .operation_release(operation.op_hash)
                        .await
                        .map_err(wasmtime::Error::msg)?;
                }
                Ok(())
            })
        },
    )?;
    scope.func_wrap_async(
        "release",
        |store: StoreContextMut<'_, WorkflowState>, (key,): (String,)| {
            Box::new(async move {
                // The result is already checkpointed, so a failed release only
                // leaves a stale continuation that instance cleanup removes.
                let released = match runtime(&store) {
                    Ok(host) => host.operation_release(operation_hash(&key)).await,
                    Err(error) => Err(error),
                };
                if let Err(error) = released {
                    tracing::warn!(%error, "operation release failed");
                }
                Ok(())
            })
        },
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_operation_hash_is_the_sha256_of_the_checkpoint_key() {
        assert_eq!(
            operation_hash(""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_ne!(operation_hash("a::1"), operation_hash("a::2"));
    }
}
