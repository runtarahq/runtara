// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Host side of `runtara:control`.
//!
//! The host cannot tell which composed component made a call, so
//! `runtara:control/api` is real only in the fresh stores the
//! [`ControlExecutor`](crate::control_executor::ControlExecutor) runs approved
//! control bytes in. Every other store — workflow roots and the agent linker
//! used by the dispatcher and the trusted executor — links `denied` stubs for
//! both `api` and `executor`, so no component, composed or not, reaches the
//! control service except through the host executor.
//!
//! The tenant, caller instance and operation of a call come from the store
//! ([`ControlAuthority`]), never from WIT arguments.

use std::sync::Arc;

use wasmtime::component::Linker;

/// WIT mirror of `runtara:control/types.error-code`.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    wasmtime::component::ComponentType,
    wasmtime::component::Lift,
    wasmtime::component::Lower,
)]
#[component(enum)]
#[repr(u8)]
pub enum ControlErrorCode {
    #[component(name = "denied")]
    Denied,
    #[component(name = "invalid")]
    Invalid,
    #[component(name = "not-found")]
    NotFound,
    #[component(name = "too-large")]
    TooLarge,
    #[component(name = "unavailable")]
    Unavailable,
    #[component(name = "unsupported")]
    Unsupported,
}

/// WIT mirror of `runtara:control/types.control-error`.
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
pub struct ControlError {
    pub code: ControlErrorCode,
    pub message: String,
}

impl ControlError {
    pub fn new(code: ControlErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    fn denied() -> Self {
        Self::new(
            ControlErrorCode::Denied,
            "control is available only through the approved control agent",
        )
    }

    fn unsupported() -> Self {
        Self::new(
            ControlErrorCode::Unsupported,
            "this control operation is not available",
        )
    }
}

/// WIT mirror of `runtara:control/types.wait-mode`.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    wasmtime::component::ComponentType,
    wasmtime::component::Lift,
    wasmtime::component::Lower,
)]
#[component(enum)]
#[repr(u8)]
pub enum WaitMode {
    #[component(name = "all")]
    All,
    #[component(name = "any")]
    Any,
}

/// WIT mirror of `runtara:control/types.wait-progress`.
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
pub struct WaitProgress {
    pub finished: Vec<String>,
    pub remaining: Vec<String>,
}

/// WIT mirror of `runtara:control/types.wait-poll`.
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
pub enum WaitPoll {
    #[component(name = "pending")]
    Pending(WaitProgress),
    #[component(name = "settled")]
    Settled(WaitProgress),
}

/// Who a control call acts for. Only the host fills it in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlAuthority {
    /// The tenant the calling run belongs to.
    pub tenant: String,
    /// The calling instance, when the call runs inside one.
    pub caller: Option<String>,
    /// The entered operation (`op_hash`), when the call site is scoped.
    pub operation: Option<String>,
}

/// The native control service behind `runtara:control/api`. Every operation
/// defaults to `unsupported` until its slice lands.
#[async_trait::async_trait]
pub trait ControlHost: Send + Sync {
    /// Register a wait on `instance_ids`; returns its id.
    async fn wait(
        &self,
        _authority: &ControlAuthority,
        _instance_ids: Vec<String>,
        _mode: WaitMode,
    ) -> Result<String, ControlError> {
        Err(ControlError::unsupported())
    }

    /// Read a registered wait without blocking.
    async fn poll_wait(
        &self,
        _authority: &ControlAuthority,
        _wait_id: String,
    ) -> Result<WaitPoll, ControlError> {
        Err(ControlError::unsupported())
    }
}

/// The real control service for one executor store.
#[derive(Clone)]
pub(crate) struct ControlApiCall {
    pub(crate) host: Arc<dyn ControlHost>,
    pub(crate) authority: ControlAuthority,
}

/// Store data that may carry a real control service.
pub(crate) trait ControlApiView {
    fn control_api(&self) -> Option<ControlApiCall>;
}

impl ControlApiView for crate::host_state::HostState {
    fn control_api(&self) -> Option<ControlApiCall> {
        self.control_api.clone()
    }
}

/// Bind `runtara:control/api` to the store's [`ControlApiCall`]. Only the
/// control executor's linker uses this; a store without one is `denied`.
pub(crate) fn add_control_api_to_linker<T: ControlApiView + Send + 'static>(
    linker: &mut Linker<T>,
) -> anyhow::Result<()> {
    let mut api = linker.instance(runtara_workflow_wit::CONTROL_API_INTERFACE_NAME)?;
    api.func_wrap_concurrent(
        "wait",
        |accessor, (instance_ids, mode): (Vec<String>, WaitMode)| {
            let call = accessor.with(|mut access| access.get().control_api());
            Box::pin(async move {
                let result = match call {
                    Some(call) => call.host.wait(&call.authority, instance_ids, mode).await,
                    None => Err(ControlError::denied()),
                };
                Ok((result,))
            })
        },
    )?;
    api.func_wrap_concurrent("poll-wait", |accessor, (wait_id,): (String,)| {
        let call = accessor.with(|mut access| access.get().control_api());
        Box::pin(async move {
            let result = match call {
                Some(call) => call.host.poll_wait(&call.authority, wait_id).await,
                None => Err(ControlError::denied()),
            };
            Ok((result,))
        })
    })?;
    Ok(())
}

/// Link `denied` stubs for `runtara:control/api`, for every store that is not
/// a control executor store.
pub(crate) fn add_denied_control_api_to_linker<T: Send + 'static>(
    linker: &mut Linker<T>,
) -> anyhow::Result<()> {
    let mut api = linker.instance(runtara_workflow_wit::CONTROL_API_INTERFACE_NAME)?;
    api.func_wrap_concurrent("wait", |_, (_, _): (Vec<String>, WaitMode)| {
        Box::pin(async { Ok((Err::<String, _>(ControlError::denied()),)) })
    })?;
    api.func_wrap_concurrent("poll-wait", |_, (_,): (String,)| {
        Box::pin(async { Ok((Err::<WaitPoll, _>(ControlError::denied()),)) })
    })?;
    Ok(())
}

/// The `error-info` a `denied` control executor call returns.
pub(crate) fn denied_error_info(message: &str) -> crate::ErrorInfo {
    control_error_info("CONTROL_DENIED", message)
}

/// A permanent, non-retryable `error-info`.
pub(crate) fn control_error_info(code: &str, message: &str) -> crate::ErrorInfo {
    crate::ErrorInfo {
        code: code.into(),
        message: message.into(),
        category: "permanent".into(),
        severity: "error".into(),
        retryable: false,
        retry_after_ms: None,
        attributes: None,
    }
}

/// Link a `denied` stub for `runtara:control/executor`: the agent linker (its
/// stores are never a workflow's) and the executor's own stores (no nesting).
pub(crate) fn add_denied_control_executor_to_linker<T: Send + 'static>(
    linker: &mut Linker<T>,
) -> anyhow::Result<()> {
    linker
        .instance(runtara_workflow_wit::CONTROL_EXECUTOR_INTERFACE_NAME)?
        .func_wrap_concurrent("invoke", |_, (_, _): (String, Vec<u8>)| {
            Box::pin(async {
                Ok((Err::<crate::operation_scope_host::SuspendableOutcome, _>(
                    denied_error_info("control executor calls are not available in this context"),
                ),))
            })
        })?;
    Ok(())
}
