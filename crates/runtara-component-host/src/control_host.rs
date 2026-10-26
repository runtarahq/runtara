// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! The control service behind the workflow API.
//!
//! The control service behind the workflow API's `/v1/instances`
//! (`control_host::json`). The tenant, caller instance and operation of a
//! call come from the launch's binding ([`ControlAuthority`]), never from the
//! request.

/// The control service's request and response types.
pub mod types;

pub use types::{
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

    fn unsupported() -> Self {
        Self::new(
            ControlErrorCode::Unsupported,
            "this control operation is not available",
        )
    }
}

/// JSON forms of these records for the workflow API.
pub mod json;

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

/// The host bound of one control call.
pub(crate) const CALL_TIME_LIMIT: std::time::Duration =
    std::time::Duration::from_millis(runtara_control_contract::CALL_TIME_LIMIT_MS);

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

#[cfg(test)]
mod tests {
    use super::*;

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
