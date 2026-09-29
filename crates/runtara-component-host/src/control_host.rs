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
            "../runtara-wit/wit/agent",
            "../runtara-wit/wit/control",
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
    SendSignalResult, SignalScope, SortField, SortOrder, StartRequest, StartResult, StateRead,
    SuspensionReason, TerminalResult,
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
            "control is available only to the control agent of a run",
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
/// defaults to `unsupported` until its slice lands, so `runtara:control@1.0.0`
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

    /// Read one instance's published state.
    async fn get_state(
        &self,
        _authority: &ControlAuthority,
        _instance_id: String,
    ) -> Result<StateRead, ControlError> {
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
}

/// The real control service for one call, with its authority and bound.
#[derive(Clone)]
pub(crate) struct ControlApiCall {
    pub(crate) host: Arc<dyn ControlHost>,
    pub(crate) authority: ControlAuthority,
    /// When the call fails with `timeout`; `None` leaves the bound to the
    /// store's own deadline.
    pub(crate) deadline: Option<tokio::time::Instant>,
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

/// The host bound of one control call.
pub(crate) const CALL_TIME_LIMIT: std::time::Duration =
    std::time::Duration::from_millis(runtara_control_contract::EXECUTION_TIME_LIMIT_MS);

/// A run's control service: the host plus the run's host-supplied identity.
/// Present only in a store invoked as the run's own prepared entry.
#[derive(Clone)]
pub(crate) struct RunControl {
    host: Arc<dyn ControlHost>,
    tenant: String,
    caller: String,
}

impl RunControl {
    /// The control service of the run `instance` of `tenant`, both
    /// host-supplied and non-empty, when the entry `enabled` it.
    pub(crate) fn for_run(
        enabled: bool,
        host: Option<Arc<dyn ControlHost>>,
        tenant: Option<&str>,
        instance: Option<&str>,
    ) -> Option<Self> {
        match (enabled, host, tenant, instance) {
            (true, Some(host), Some(tenant), Some(caller))
                if !tenant.is_empty() && !caller.is_empty() =>
            {
                Some(Self {
                    host,
                    tenant: tenant.to_owned(),
                    caller: caller.to_owned(),
                })
            }
            _ => None,
        }
    }

    /// One call acting for the run under the entered `operation`, bounded
    /// by `deadline` and [`CALL_TIME_LIMIT`].
    pub(crate) fn call(
        &self,
        operation: Option<String>,
        deadline: tokio::time::Instant,
    ) -> ControlApiCall {
        ControlApiCall {
            host: Arc::clone(&self.host),
            authority: ControlAuthority {
                tenant: self.tenant.clone(),
                caller: Some(self.caller.clone()),
                operation,
            },
            deadline: Some(deadline.min(tokio::time::Instant::now() + CALL_TIME_LIMIT)),
        }
    }
}

/// Logs routing identity and outcome of one call, never inputs or outputs.
/// Dropped before [`Self::finish`], it logs `cancelled`.
struct CallAudit<'a> {
    authority: &'a ControlAuthority,
    function: &'static str,
    started: std::time::Instant,
    outcome: &'static str,
}

impl<'a> CallAudit<'a> {
    fn start(authority: &'a ControlAuthority, function: &'static str) -> Self {
        Self {
            authority,
            function,
            started: std::time::Instant::now(),
            outcome: "cancelled",
        }
    }

    fn finish<T>(mut self, result: &Result<T, ControlError>) {
        self.outcome = if result.is_ok() { "success" } else { "failure" };
    }
}

impl Drop for CallAudit<'_> {
    fn drop(&mut self) {
        tracing::info!(
            tenant = %self.authority.tenant,
            caller = self.authority.caller.as_deref(),
            capability = self.function,
            duration_ms = self.started.elapsed().as_millis() as u64,
            outcome = self.outcome,
            "control capability completed"
        );
    }
}

/// `work` under `deadline`: past it, the call fails with `timeout`.
async fn bounded<T>(
    deadline: Option<tokio::time::Instant>,
    work: impl std::future::Future<Output = Result<T, ControlError>>,
) -> Result<T, ControlError> {
    match deadline {
        Some(deadline) => tokio::time::timeout_at(deadline, work)
            .await
            .unwrap_or_else(|_| {
                Err(ControlError::new(
                    ControlErrorCode::Timeout,
                    "the control call ran past its deadline",
                ))
            }),
        None => work.await,
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
            "get-state" => get_state(String) -> StateRead,
            "query" => query(QueryRequest) -> InstancePage,
            "list-pending-signals" => list_pending_signals(PendingSignalsRequest) -> PendingSignalPage,
            "send-signal" => send_signal(SendSignalRequest) -> SendSignalResult,
            "cancel" => cancel(CancelRequest) -> CommandResult,
            "pause" => pause(String) -> CommandResult,
            "resume" => resume(String) -> CommandResult,
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
pub(crate) const LINKED_API_FUNCTIONS: [&str; 9] = with_control_api!(api_names!(unused));

/// Bind `runtara:control/api` to the store's [`ControlApiCall`]; a store
/// without one is `denied`. Each call is bounded and audited.
pub(crate) fn add_control_api_to_linker<T: ControlApiView + Send + 'static>(
    linker: &mut Linker<T>,
) -> anyhow::Result<()> {
    let mut api = linker.instance(runtara_wit::control::API)?;
    macro_rules! link_real {
        ($api:ident; $($name:literal => $method:ident($param:ty) -> $ok:ty,)*) => {$(
            $api.func_wrap_concurrent($name, |accessor, (param,): ($param,)| {
                let call = accessor.with(|mut access| access.get().control_api());
                Box::pin(async move {
                    let result: Result<$ok, ControlError> = match call {
                        Some(call) => {
                            let audit = CallAudit::start(&call.authority, $name);
                            let result = bounded(
                                call.deadline,
                                call.host.$method(&call.authority, param),
                            )
                            .await;
                            audit.finish(&result);
                            result
                        }
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
    let mut api = linker.instance(runtara_wit::control::API)?;
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
        details: None,
    }
}

/// Link a `denied` stub for `runtara:control/executor`: the agent linker (its
/// stores are never a workflow's) and the executor's own stores (no nesting).
pub(crate) fn add_denied_control_executor_to_linker<T: Send + 'static>(
    linker: &mut Linker<T>,
) -> anyhow::Result<()> {
    linker
        .instance(runtara_wit::control::EXECUTOR)?
        .func_wrap_concurrent("invoke", |_, (_, _): (String, Vec<u8>)| {
            Box::pin(async {
                Ok((Err::<Vec<u8>, _>(denied_error_info(
                    "control executor calls are not available in this context",
                )),))
            })
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The host links exactly the functions the frozen WIT declares.
    #[test]
    fn every_api_function_is_linked() {
        let wit = runtara_wit::control::WIT;
        let api = &wit[wit.find("interface api {").expect("api interface")..];
        let api = &api[..api.find("\n}").expect("end of api")];
        let declared: Vec<&str> = api
            .lines()
            .filter_map(|line| line.trim().split_once(": async func"))
            .map(|(name, _)| name)
            .collect();
        assert_eq!(declared, LINKED_API_FUNCTIONS);
    }

    struct Service;

    #[async_trait::async_trait]
    impl ControlHost for Service {}

    fn host() -> Option<Arc<dyn ControlHost>> {
        Some(Arc::new(Service))
    }

    #[test]
    fn only_an_enabled_entry_with_a_host_and_run_identity_gets_control() {
        assert!(RunControl::for_run(true, host(), Some("t"), Some("run")).is_some());
        for (enabled, host, tenant, instance) in [
            (false, host(), Some("t"), Some("run")),
            (true, None, Some("t"), Some("run")),
            (true, host(), None, Some("run")),
            (true, host(), Some(""), Some("run")),
            (true, host(), Some("t"), None),
            (true, host(), Some("t"), Some("")),
        ] {
            assert!(RunControl::for_run(enabled, host, tenant, instance).is_none());
        }
    }

    #[tokio::test]
    async fn a_call_acts_for_the_run_within_its_bound() {
        let control = RunControl::for_run(true, host(), Some("t"), Some("run")).unwrap();
        let far = tokio::time::Instant::now() + std::time::Duration::from_secs(3600);
        let call = control.call(Some("op".into()), far);
        assert_eq!(
            call.authority,
            ControlAuthority {
                tenant: "t".into(),
                caller: Some("run".into()),
                operation: Some("op".into()),
            }
        );
        let bound = call.deadline.unwrap();
        assert!(bound <= tokio::time::Instant::now() + CALL_TIME_LIMIT);
        let near = tokio::time::Instant::now() + std::time::Duration::from_secs(1);
        assert_eq!(control.call(None, near).deadline, Some(near));
    }

    #[tokio::test]
    async fn a_call_past_its_deadline_times_out() {
        let past = tokio::time::Instant::now();
        let error = bounded::<()>(Some(past), std::future::pending())
            .await
            .unwrap_err();
        assert_eq!(error.code, ControlErrorCode::Timeout);
        assert_eq!(bounded(None, async { Ok(7) }).await.unwrap(), 7);
    }
}
