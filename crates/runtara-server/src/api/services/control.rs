//! Native control service behind `runtara:control/api`.
//!
//! The component host runs the approved control agent in fresh stores and
//! hands every `api` call here with the caller's authority (tenant, calling
//! instance, operation) taken from the calling run, never from arguments.
//! This slice serves the reads — `get`, `query`, `list-pending-signals` —
//! across the caller's tenant. Identity operations and caller-relative
//! filters answer `requires-instance` without a calling instance and
//! `unsupported` otherwise until their slices land.
//!
//! The service is late-bound: the executor exists before the embedded
//! runtime does, so a call waits up to [`INSTALL_WAIT`] for
//! [`NativeControl::install`] and is `unavailable` after that.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use runtara_component_host::control_host::{
    CancelRequest, CommandResult, ControlAuthority, ControlError, ControlErrorCode, ControlHost,
    InstanceDetail, InstancePage, InstanceStatus, InstanceSummary, ParentFilter, PendingSignal,
    PendingSignalPage, PendingSignalsRequest, QueryRequest, SendSignalRequest, SendSignalResult,
    SignalScope, SortField, SortOrder, StartRequest, StartResult, SuspensionReason, TerminalResult,
    WaitPoll, WaitRequest,
};
use runtara_control_contract as contract;
use runtara_environment::control_reads::ControlInstance;
use runtara_environment::instance_repository::ListInstancesOptions;
use serde_json::{Value, json};

use crate::runtime_client::RuntimeClient;

/// How long a call waits for the embedded runtime to install the service.
pub const INSTALL_WAIT: Duration = Duration::from_secs(30);

/// Longest instance id, workflow id or filter value control accepts.
const MAX_ID_BYTES: usize = 256;

/// The native control service. Construct once, hand it to the control
/// executor, and [`install`](Self::install) the runtime once it exists.
pub struct NativeControl {
    runtime: tokio::sync::watch::Sender<Option<Arc<RuntimeClient>>>,
    /// The only tenant this process serves; `None` accepts any non-empty
    /// tenant (tests).
    tenant: Option<String>,
    install_wait: Duration,
}

impl NativeControl {
    /// A service for `tenant` that waits [`INSTALL_WAIT`] for its runtime.
    pub fn new(tenant: Option<String>) -> Self {
        Self::with_install_wait(tenant, INSTALL_WAIT)
    }

    /// [`Self::new`] with a custom install wait.
    pub fn with_install_wait(tenant: Option<String>, install_wait: Duration) -> Self {
        Self {
            runtime: tokio::sync::watch::channel(None).0,
            tenant,
            install_wait,
        }
    }

    /// Bind the embedded runtime; calls waiting for it proceed.
    pub fn install(&self, runtime: Arc<RuntimeClient>) {
        self.runtime.send_replace(Some(runtime));
    }

    async fn runtime(&self) -> Result<Arc<RuntimeClient>, ControlError> {
        let mut receiver = self.runtime.subscribe();
        let installed = tokio::time::timeout(
            self.install_wait,
            receiver.wait_for(|runtime| runtime.is_some()),
        )
        .await;
        match installed {
            Ok(Ok(runtime)) => Ok(runtime.clone().expect("waited for an installed runtime")),
            _ => Err(unavailable("the control service is not ready yet")),
        }
    }

    /// The tenant of the call, which must be this process's.
    fn tenant<'a>(&self, authority: &'a ControlAuthority) -> Result<&'a str, ControlError> {
        let tenant = authority.tenant.as_str();
        if tenant.is_empty() || self.tenant.as_deref().is_some_and(|own| own != tenant) {
            return Err(ControlError::new(
                ControlErrorCode::Denied,
                "the call's tenant is not served here",
            ));
        }
        Ok(tenant)
    }
}

fn unavailable(message: &str) -> ControlError {
    ControlError {
        code: ControlErrorCode::Unavailable,
        message: message.into(),
        retry_after_ms: Some(1_000),
    }
}

fn invalid(message: impl Into<String>) -> ControlError {
    ControlError::new(ControlErrorCode::Invalid, message)
}

fn not_found() -> ControlError {
    ControlError::new(ControlErrorCode::NotFound, "no such run in this tenant")
}

/// Identity operations and caller-relative filters: `requires-instance`
/// without a calling instance, else not available in this build.
fn identity_call(authority: &ControlAuthority, what: &str) -> ControlError {
    if authority.caller.is_none() {
        ControlError::new(
            ControlErrorCode::RequiresInstance,
            format!("{what} needs a calling run"),
        )
    } else {
        ControlError::new(
            ControlErrorCode::Unsupported,
            format!("{what} is not available in this build"),
        )
    }
}

fn check_id(field: &str, value: &str) -> Result<(), ControlError> {
    if value.trim().is_empty() || value.len() > MAX_ID_BYTES {
        return Err(invalid(format!("{field} must be 1-{MAX_ID_BYTES} bytes")));
    }
    Ok(())
}

fn check_page_size(page_size: u32) -> Result<i64, ControlError> {
    if !(contract::PAGE_SIZE_MIN..=contract::PAGE_SIZE_MAX).contains(&page_size) {
        return Err(invalid(format!(
            "pageSize must be {}-{}",
            contract::PAGE_SIZE_MIN,
            contract::PAGE_SIZE_MAX
        )));
    }
    Ok(i64::from(page_size))
}

/// Page tokens are opaque to callers; here they are the next offset.
fn page_offset(token: Option<&str>) -> Result<u64, ControlError> {
    match token {
        None => Ok(0),
        Some(token) => token
            .parse::<u64>()
            .ok()
            .filter(|offset| *offset <= i64::MAX as u64)
            .ok_or_else(|| invalid("pageToken is not one this service issued")),
    }
}

fn time(field: &str, ms: Option<u64>) -> Result<Option<DateTime<Utc>>, ControlError> {
    ms.map(|ms| {
        i64::try_from(ms)
            .ok()
            .and_then(DateTime::from_timestamp_millis)
            .ok_or_else(|| invalid(format!("{field} is out of range")))
    })
    .transpose()
}

fn millis(at: DateTime<Utc>) -> u64 {
    at.timestamp_millis().max(0) as u64
}

fn status(status: runtara_core::domain::InstanceStatus) -> InstanceStatus {
    use runtara_core::domain::InstanceStatus as Core;
    match status {
        Core::Pending => InstanceStatus::Pending,
        Core::Running => InstanceStatus::Running,
        Core::Suspended => InstanceStatus::Suspended,
        Core::Completed => InstanceStatus::Completed,
        Core::Failed => InstanceStatus::Failed,
        Core::Cancelled => InstanceStatus::Cancelled,
    }
}

/// The stored statuses a requested control status matches. `queued` and
/// `not-started` describe admission, which no run reaches yet.
fn stored_status(status: InstanceStatus) -> Option<&'static str> {
    match status {
        InstanceStatus::Pending => Some("pending"),
        InstanceStatus::Running => Some("running"),
        InstanceStatus::Suspended => Some("suspended"),
        InstanceStatus::Completed => Some("completed"),
        InstanceStatus::Failed => Some("failed"),
        InstanceStatus::Cancelled => Some("cancelled"),
        InstanceStatus::Queued | InstanceStatus::NotStarted => None,
    }
}

fn suspension_reason(row: &ControlInstance) -> Option<SuspensionReason> {
    if row.status != runtara_core::domain::InstanceStatus::Suspended {
        return None;
    }
    if row.explicitly_paused {
        return Some(SuspensionReason::Paused);
    }
    match row.termination_reason.as_deref() {
        Some("waiting_signal") => Some(SuspensionReason::WaitingSignal),
        Some("sleeping") => Some(SuspensionReason::Sleeping),
        Some("shutdown_requested" | "environment_restart") => Some(SuspensionReason::Shutdown),
        _ => None,
    }
}

fn summary(row: &ControlInstance) -> InstanceSummary {
    let (workflow_id, version) = row
        .image_name
        .as_deref()
        .map(crate::workers::runtara_dto::parse_image_id)
        .unwrap_or_default();
    InstanceSummary {
        instance_id: row.instance_id.clone(),
        workflow_id,
        version: u32::try_from(version).ok().filter(|version| *version > 0),
        run_label: row.run_label.clone(),
        // The parent link lands with `start`.
        parent_instance_id: None,
        status: status(row.status),
        suspension_reason: suspension_reason(row),
        termination_reason: row
            .status
            .is_terminal()
            .then(|| row.termination_reason.clone())
            .flatten(),
        created_at_ms: millis(row.created_at),
        started_at_ms: row.started_at.map(millis),
        finished_at_ms: row.finished_at.map(millis),
    }
}

/// The terminal result of a run read with the `get` caps: values over their
/// cap are omitted and flagged, never truncated.
fn terminal(row: &ControlInstance) -> TerminalResult {
    if !row.status.is_terminal() {
        return TerminalResult {
            output: None,
            output_bytes: None,
            output_omitted: false,
            error: None,
            error_omitted: false,
        };
    }
    // The error column is text; control hands out JSON, so a message that is
    // not JSON travels as a JSON string.
    let error = row.error.as_deref().map(|text| {
        serde_json::from_str::<Value>(text)
            .unwrap_or_else(|_| Value::String(text.to_owned()))
            .to_string()
            .into_bytes()
    });
    let error_omitted = row.error_bytes.is_some() && error.is_none();
    // The JSON-string wrapping can grow an error past its inline cap.
    let (error, error_omitted) = match error {
        Some(bytes) if bytes.len() > contract::GET_ERROR_INLINE_BYTES => (None, true),
        other => (other, error_omitted),
    };
    TerminalResult {
        output_omitted: row.output_bytes.is_some() && row.output.is_none(),
        output: row.output.clone(),
        output_bytes: row.output_bytes,
        error,
        error_omitted,
    }
}

fn input_error(error: runtara_core::persistence::inputs::InputError) -> ControlError {
    use runtara_core::persistence::inputs::InputError;
    match error {
        InputError::NotFound => not_found(),
        _ => unavailable("pending signals are unavailable"),
    }
}

fn pending_signal(
    request: runtara_core::persistence::inputs::InputRequest,
    workflow_id: &str,
) -> PendingSignal {
    let metadata = &request.spec.metadata;
    let text = |key: &str| {
        metadata
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let context = json!({
        "stepName": metadata.get("step_name").cloned().unwrap_or(Value::Null),
        "context": metadata.get("context").cloned().unwrap_or(Value::Null),
        "correlation": metadata.get("correlation").cloned().unwrap_or(Value::Null),
    });
    PendingSignal {
        instance_id: request.instance_id,
        workflow_id: workflow_id.to_owned(),
        signal_id: text("step_id").unwrap_or(request.spec.signal_id),
        request_id: request.request_id,
        action_key: text("action_key"),
        response_schema: request
            .spec
            .response_schema
            .as_ref()
            .map(|schema| schema.to_string().into_bytes()),
        context: Some(context.to_string().into_bytes()),
        requested_at_ms: millis(request.created_at),
        deadline_ms: request.spec.deadline.map(millis),
    }
}

/// Rough wire size of a pending signal, for the response cap.
fn signal_bytes(signal: &PendingSignal) -> usize {
    signal.instance_id.len()
        + signal.workflow_id.len()
        + signal.signal_id.len()
        + signal.request_id.len()
        + signal.action_key.as_ref().map_or(0, String::len)
        + signal.response_schema.as_ref().map_or(0, Vec::len)
        + signal.context.as_ref().map_or(0, Vec::len)
        + 64
}

#[async_trait::async_trait]
impl ControlHost for NativeControl {
    async fn start(
        &self,
        authority: &ControlAuthority,
        _request: StartRequest,
    ) -> Result<StartResult, ControlError> {
        Err(identity_call(authority, "start"))
    }

    async fn get(
        &self,
        authority: &ControlAuthority,
        instance_id: String,
    ) -> Result<InstanceDetail, ControlError> {
        let tenant = self.tenant(authority)?;
        check_id("instanceId", &instance_id)?;
        let runtime = self.runtime().await?;
        let row = runtime
            .control_instance(
                tenant,
                &instance_id,
                contract::GET_OUTPUT_INLINE_BYTES,
                contract::GET_ERROR_INLINE_BYTES,
            )
            .await
            .map_err(|_| unavailable("the run could not be read"))?
            .ok_or_else(not_found)?;
        Ok(InstanceDetail {
            instance: summary(&row),
            terminal: terminal(&row),
        })
    }

    async fn query(
        &self,
        authority: &ControlAuthority,
        request: QueryRequest,
    ) -> Result<InstancePage, ControlError> {
        let tenant = self.tenant(authority)?;
        let limit = check_page_size(request.page_size)?;
        let offset = page_offset(request.page_token.as_deref())?;
        match &request.parent {
            None => {}
            Some(ParentFilter::Caller) => return Err(identity_call(authority, "a caller filter")),
            Some(ParentFilter::Instance(_)) => {
                return Err(ControlError::new(
                    ControlErrorCode::Unsupported,
                    "the parent filter is not available in this build",
                ));
            }
        }
        if let Some(workflow) = &request.workflow_id {
            check_id("workflowId", workflow)?;
        }
        if let Some(label) = &request.run_label
            && runtara_dsl::run_label::normalize_run_label(Some(label)).is_err()
        {
            return Err(invalid("runLabel is not a valid run label"));
        }
        let statuses: Vec<String> = request
            .statuses
            .iter()
            .filter_map(|status| stored_status(*status))
            .map(str::to_owned)
            .collect();
        if !request.statuses.is_empty() && statuses.is_empty() {
            // Only admission states, which no run is in yet.
            return Ok(InstancePage {
                items: Vec::new(),
                total: 0,
                next_page_token: None,
            });
        }
        let order_by = match (request.sort_by, request.order) {
            (SortField::CreatedAt, SortOrder::Ascending) => "created_at_asc",
            (SortField::CreatedAt, SortOrder::Descending) => "created_at_desc",
            (SortField::FinishedAt, SortOrder::Ascending) => "finished_at_asc",
            (SortField::FinishedAt, SortOrder::Descending) => "finished_at_desc",
        };
        let options = ListInstancesOptions {
            tenant_id: Some(tenant.to_owned()),
            run_label: request.run_label.clone(),
            statuses: (!statuses.is_empty()).then_some(statuses),
            image_name_prefix: request
                .workflow_id
                .as_ref()
                .map(|workflow| format!("{workflow}:")),
            created_after: time("createdAfterMs", request.created_after_ms)?,
            created_before: time("createdBeforeMs", request.created_before_ms)?,
            finished_after: time("finishedAfterMs", request.finished_after_ms)?,
            finished_before: time("finishedBeforeMs", request.finished_before_ms)?,
            order_by: Some(order_by.into()),
            limit,
            offset: offset as i64,
            ..Default::default()
        };
        let runtime = self.runtime().await?;
        let (rows, total) = runtime
            .control_instances(&options)
            .await
            .map_err(|_| unavailable("runs could not be listed"))?;
        let total = total.max(0) as u64;
        let next = offset + rows.len() as u64;
        Ok(InstancePage {
            items: rows.iter().map(summary).collect(),
            total,
            next_page_token: (next < total && !rows.is_empty()).then(|| next.to_string()),
        })
    }

    async fn list_pending_signals(
        &self,
        authority: &ControlAuthority,
        request: PendingSignalsRequest,
    ) -> Result<PendingSignalPage, ControlError> {
        let tenant = self.tenant(authority)?;
        let limit = check_page_size(request.page_size)? as u32;
        let offset = page_offset(request.page_token.as_deref())?;
        for (field, value) in [
            ("signalId", request.signal_id.as_deref()),
            ("actionKey", request.action_key.as_deref()),
        ] {
            if let Some(value) = value {
                check_id(field, value)?;
            }
        }
        let runtime = self.runtime().await?;
        let (instances, workflow_id) = match &request.scope {
            SignalScope::Children => return Err(identity_call(authority, "a children scope")),
            SignalScope::Instance(id) => {
                check_id("instanceId", id)?;
                let row = runtime
                    .control_instance(tenant, id, 0, 0)
                    .await
                    .map_err(|_| unavailable("the run could not be read"))?
                    .ok_or_else(not_found)?;
                (vec![id.clone()], summary(&row).workflow_id)
            }
            SignalScope::Workflow(workflow) => {
                check_id("workflowId", workflow)?;
                let instances = runtime
                    .workflow_input_instances(tenant, workflow)
                    .await
                    .map_err(input_error)?;
                (instances, workflow.clone())
            }
        };
        if instances.is_empty() {
            return Ok(PendingSignalPage {
                items: Vec::new(),
                next_page_token: None,
            });
        }
        let filtered = request.signal_id.is_some() || request.action_key.is_some();
        // Filters apply before pagination, so a filtered scope is read whole.
        let (page_offset, page_limit) = if filtered {
            (0, u32::MAX)
        } else {
            (offset, limit)
        };
        let page = runtime
            .list_input_requests(tenant, &instances, page_offset, page_limit)
            .await
            .map_err(input_error)?;
        let total = page.total_count;
        let mut signals: Vec<PendingSignal> = page
            .requests
            .into_iter()
            .map(|request| pending_signal(request, &workflow_id))
            .collect();
        let (items, next) = if filtered {
            signals.retain(|signal| {
                request
                    .signal_id
                    .as_ref()
                    .is_none_or(|id| &signal.signal_id == id)
                    && request
                        .action_key
                        .as_ref()
                        .is_none_or(|key| signal.action_key.as_ref() == Some(key))
            });
            let matched = signals.len() as u64;
            let items: Vec<_> = signals
                .into_iter()
                .skip(offset as usize)
                .take(limit as usize)
                .collect();
            let next = offset + items.len() as u64;
            (items, (next < matched).then_some(next))
        } else {
            let next = offset + signals.len() as u64;
            let more = next < total && !signals.is_empty();
            (signals, more.then_some(next))
        };
        if items.iter().map(signal_bytes).sum::<usize>() > contract::MAX_RESPONSE_BYTES {
            return Err(ControlError::new(
                ControlErrorCode::TooLarge,
                "the page is over the response cap; request a smaller pageSize",
            ));
        }
        Ok(PendingSignalPage {
            items,
            next_page_token: next.map(|next| next.to_string()),
        })
    }

    async fn send_signal(
        &self,
        authority: &ControlAuthority,
        _request: SendSignalRequest,
    ) -> Result<SendSignalResult, ControlError> {
        Err(identity_call(authority, "send-signal"))
    }

    async fn cancel(
        &self,
        authority: &ControlAuthority,
        _request: CancelRequest,
    ) -> Result<CommandResult, ControlError> {
        Err(identity_call(authority, "cancel"))
    }

    async fn pause(
        &self,
        authority: &ControlAuthority,
        _instance_id: String,
    ) -> Result<CommandResult, ControlError> {
        Err(identity_call(authority, "pause"))
    }

    async fn resume(
        &self,
        authority: &ControlAuthority,
        _instance_id: String,
    ) -> Result<CommandResult, ControlError> {
        Err(identity_call(authority, "resume"))
    }

    async fn wait(
        &self,
        authority: &ControlAuthority,
        _request: WaitRequest,
    ) -> Result<String, ControlError> {
        Err(identity_call(authority, "wait"))
    }

    async fn poll_wait(
        &self,
        authority: &ControlAuthority,
        _wait_id: String,
    ) -> Result<WaitPoll, ControlError> {
        Err(identity_call(authority, "poll-wait"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn authority(caller: Option<&str>) -> ControlAuthority {
        ControlAuthority {
            tenant: "tenant".into(),
            caller: caller.map(str::to_owned),
            operation: None,
        }
    }

    #[test]
    fn identity_calls_need_a_caller_then_wait_for_their_slice() {
        assert_eq!(
            identity_call(&authority(None), "start").code,
            ControlErrorCode::RequiresInstance
        );
        assert_eq!(
            identity_call(&authority(Some("parent")), "start").code,
            ControlErrorCode::Unsupported
        );
    }

    #[test]
    fn page_sizes_and_tokens_are_validated() {
        assert!(check_page_size(0).is_err());
        assert!(check_page_size(101).is_err());
        assert_eq!(check_page_size(100).unwrap(), 100);
        assert_eq!(page_offset(None).unwrap(), 0);
        assert_eq!(page_offset(Some("40")).unwrap(), 40);
        assert!(page_offset(Some("next")).is_err());
    }

    #[tokio::test]
    async fn an_uninstalled_service_is_unavailable_and_foreign_tenants_denied() {
        let control = NativeControl::with_install_wait(Some("tenant".into()), Duration::ZERO);
        let error = control
            .get(&authority(None), "run-1".into())
            .await
            .unwrap_err();
        assert_eq!(error.code, ControlErrorCode::Unavailable);
        let mut foreign = authority(None);
        foreign.tenant = "other".into();
        let error = control.get(&foreign, "run-1".into()).await.unwrap_err();
        assert_eq!(error.code, ControlErrorCode::Denied);
    }
}
