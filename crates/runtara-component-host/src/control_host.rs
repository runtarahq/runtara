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

mod bindings {
    // Types only: the host links `api` by hand below and calls `execution`
    // through a typed function, so every type is generated from the WIT
    // itself and cannot drift from it.
    wasmtime::component::bindgen!({
        path: [
            "../runtara-agent-wit/wit",
            "../runtara-agent-suspension/wit",
            "../runtara-workflow-wit/wit/control",
        ],
        world: "runtara:control/control-agent-host",
        imports: { default: async | trappable },
        exports: { default: async },
        additional_derives: [PartialEq, Eq],
    });
}

pub use bindings::runtara::control::types::{
    CancelRequest, CommandOutcome, CommandResult, ControlError, ErrorCode as ControlErrorCode,
    InstanceDetail, InstancePage, InstanceStatus, InstanceSummary, ParentClosePolicy, ParentFilter,
    PendingSignal, PendingSignalPage, PendingSignalsRequest, QueryRequest, SendSignalRequest,
    SendSignalResult, SignalScope, SortField, SortOrder, StartRequest, StartResult,
    SuspensionReason, TargetOutcome, TerminalResult, WaitMode, WaitPoll, WaitProgress, WaitRequest,
    WaitResolution, WaitSettled,
};

impl ControlError {
    pub fn new(code: ControlErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            retry_after_ms: None,
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
/// defaults to `unsupported` until its slice lands, so `runtara:control@0.1.0`
/// never needs a new version for it. The rules each operation enforces are
/// pinned in the WIT's doc comments.
#[async_trait::async_trait]
pub trait ControlHost: Send + Sync {
    /// Durably admit a child of the caller.
    async fn start(
        &self,
        _authority: &ControlAuthority,
        _request: StartRequest,
    ) -> Result<StartResult, ControlError> {
        Err(ControlError::unsupported())
    }

    /// Read one instance of the tenant.
    async fn get(
        &self,
        _authority: &ControlAuthority,
        _instance_id: String,
    ) -> Result<InstanceDetail, ControlError> {
        Err(ControlError::unsupported())
    }

    /// Find instances of the tenant.
    async fn query(
        &self,
        _authority: &ControlAuthority,
        _request: QueryRequest,
    ) -> Result<InstancePage, ControlError> {
        Err(ControlError::unsupported())
    }

    /// List open `WaitForSignal` requests.
    async fn list_pending_signals(
        &self,
        _authority: &ControlAuthority,
        _request: PendingSignalsRequest,
    ) -> Result<PendingSignalPage, ControlError> {
        Err(ControlError::unsupported())
    }

    /// Answer an open `WaitForSignal` request.
    async fn send_signal(
        &self,
        _authority: &ControlAuthority,
        _request: SendSignalRequest,
    ) -> Result<SendSignalResult, ControlError> {
        Err(ControlError::unsupported())
    }

    /// Cancel a child.
    async fn cancel(
        &self,
        _authority: &ControlAuthority,
        _request: CancelRequest,
    ) -> Result<CommandResult, ControlError> {
        Err(ControlError::unsupported())
    }

    /// Pause a child.
    async fn pause(
        &self,
        _authority: &ControlAuthority,
        _instance_id: String,
    ) -> Result<CommandResult, ControlError> {
        Err(ControlError::unsupported())
    }

    /// Resume an explicitly paused child.
    async fn resume(
        &self,
        _authority: &ControlAuthority,
        _instance_id: String,
    ) -> Result<CommandResult, ControlError> {
        Err(ControlError::unsupported())
    }

    /// Register the caller operation's wait; returns its id.
    async fn wait(
        &self,
        _authority: &ControlAuthority,
        _request: WaitRequest,
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

/// Every `runtara:control/api` function as `name => method(param) -> ok`,
/// handed to `$link!($linker; ...)`. One list for the real and the denied
/// binding, so both always link the whole interface.
macro_rules! with_control_api {
    ($link:ident!($linker:ident)) => {
        $link!($linker;
            "start" => start(StartRequest) -> StartResult,
            "get" => get(String) -> InstanceDetail,
            "query" => query(QueryRequest) -> InstancePage,
            "list-pending-signals" => list_pending_signals(PendingSignalsRequest) -> PendingSignalPage,
            "send-signal" => send_signal(SendSignalRequest) -> SendSignalResult,
            "cancel" => cancel(CancelRequest) -> CommandResult,
            "pause" => pause(String) -> CommandResult,
            "resume" => resume(String) -> CommandResult,
            "wait" => wait(WaitRequest) -> String,
            "poll-wait" => poll_wait(String) -> WaitPoll,
        )
    };
}

/// Names of the functions the host links for `runtara:control/api`.
#[cfg(test)]
macro_rules! api_names {
    ($linker:ident; $($name:literal => $method:ident($param:ty) -> $ok:ty,)*) => {
        [$($name),*]
    };
}

#[cfg(test)]
pub(crate) const LINKED_API_FUNCTIONS: [&str; 10] = with_control_api!(api_names!(unused));

/// Bind `runtara:control/api` to the store's [`ControlApiCall`]. Only the
/// control executor's linker uses this; a store without one is `denied`.
pub(crate) fn add_control_api_to_linker<T: ControlApiView + Send + 'static>(
    linker: &mut Linker<T>,
) -> anyhow::Result<()> {
    let mut api = linker.instance(runtara_workflow_wit::CONTROL_API_INTERFACE_NAME)?;
    macro_rules! link_real {
        ($api:ident; $($name:literal => $method:ident($param:ty) -> $ok:ty,)*) => {$(
            $api.func_wrap_concurrent($name, |accessor, (param,): ($param,)| {
                let call = accessor.with(|mut access| access.get().control_api());
                Box::pin(async move {
                    let result: Result<$ok, ControlError> = match call {
                        Some(call) => call.host.$method(&call.authority, param).await,
                        None => Err(ControlError::denied()),
                    };
                    Ok((result,))
                })
            })?;
        )*};
    }
    with_control_api!(link_real!(api));
    Ok(())
}

/// Link `denied` stubs for `runtara:control/api`, for every store that is not
/// a control executor store.
pub(crate) fn add_denied_control_api_to_linker<T: Send + 'static>(
    linker: &mut Linker<T>,
) -> anyhow::Result<()> {
    let mut api = linker.instance(runtara_workflow_wit::CONTROL_API_INTERFACE_NAME)?;
    macro_rules! link_denied {
        ($api:ident; $($name:literal => $method:ident($param:ty) -> $ok:ty,)*) => {$(
            $api.func_wrap_concurrent($name, |_, (_,): ($param,)| {
                Box::pin(async { Ok((Err::<$ok, _>(ControlError::denied()),)) })
            })?;
        )*};
    }
    with_control_api!(link_denied!(api));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operation_scope_host::{SuspendableOutcome, Suspension, SuspensionWake};
    use runtara_agent_suspension::layout;
    use wasmtime::component::ComponentType;

    /// The host links exactly the functions the frozen WIT declares.
    #[test]
    fn every_api_function_is_linked() {
        let wit = runtara_workflow_wit::CONTROL_WIT;
        let api = &wit[wit.find("interface api {").expect("api interface")..];
        let api = &api[..api.find("\n}").expect("end of api")];
        let declared: Vec<&str> = api
            .lines()
            .filter_map(|line| line.trim().split_once(": async func"))
            .map(|(name, _)| name)
            .collect();
        assert_eq!(declared, LINKED_API_FUNCTIONS);
    }

    /// The hand-written suspension mirrors have the canonical layout the
    /// emitter reads (`runtara_agent_suspension::layout`, pinned against the
    /// WIT by `runtara-workflow-wit`).
    #[test]
    fn suspension_mirrors_match_the_canonical_layout() {
        assert_eq!(SuspensionWake::SIZE32, layout::WAKE_SIZE as usize);
        assert_eq!(SuspensionWake::ALIGN32, layout::WAKE_ALIGN);
        assert_eq!(Suspension::SIZE32, layout::SUSPENSION_SIZE as usize);
        assert_eq!(Suspension::ALIGN32, layout::SUSPENSION_ALIGN);
        assert_eq!(SuspendableOutcome::SIZE32, layout::OUTCOME_SIZE as usize);
        assert_eq!(SuspendableOutcome::ALIGN32, layout::OUTCOME_ALIGN);
        assert_eq!(
            <Result<SuspendableOutcome, crate::ErrorInfo> as ComponentType>::ALIGN32,
            layout::INVOKE_RESULT_PAYLOAD_OFFSET
        );
    }
}
