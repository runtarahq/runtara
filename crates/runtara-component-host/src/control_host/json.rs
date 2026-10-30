//! JSON forms of the control records, for the workflow API's `/v1/instances`
//! family. Field names and enum cases are camelCase. Byte fields that carry JSON (inputs, outputs,
//! errors, state, payloads, schemas, contexts) appear as inline JSON values.

use serde_json::{Map, Value, json};

use runtara_workflow_api::{ApiError, ControlCall, ErrorCode};

use super::{
    CallAudit, CancelRequest, CommandOutcome, CommandResult, ControlAuthority, ControlError,
    ControlErrorCode, ControlHost, InstanceDetail, InstancePage, InstanceStatus, InstanceSummary,
    ParentClosePolicy, ParentFilter, PendingSignal, PendingSignalPage, PendingSignalsRequest,
    QueryRequest, SendSignalRequest, SendSignalResult, SignalScope, SortField, SortOrder,
    StartRequest, StartResult, StateRead, SuspensionReason, TerminalResult, bounded,
};

/// A request that does not have the expected shape.
pub type Invalid = String;

fn object<'a>(value: &'a Value, what: &str) -> Result<&'a Map<String, Value>, Invalid> {
    value
        .as_object()
        .ok_or_else(|| format!("{what} must be a JSON object"))
}

fn string(map: &Map<String, Value>, field: &str) -> Result<String, Invalid> {
    map.get(field)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("`{field}` is required and must be a string"))
}

fn opt_string(map: &Map<String, Value>, field: &str) -> Result<Option<String>, Invalid> {
    match map.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(format!("`{field}` must be a string")),
    }
}

fn opt_u64(map: &Map<String, Value>, field: &str) -> Result<Option<u64>, Invalid> {
    match map.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| format!("`{field}` must be a non-negative integer")),
    }
}

fn opt_u32(map: &Map<String, Value>, field: &str) -> Result<Option<u32>, Invalid> {
    opt_u64(map, field)?
        .map(|value| u32::try_from(value).map_err(|_| format!("`{field}` is out of range")))
        .transpose()
}

/// A JSON value as the bytes a WIT `list<u8>` field carries.
fn bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).expect("a JSON value always serializes")
}

/// Bytes a WIT field carries as the JSON value they hold, or a string when
/// they are not JSON.
fn value(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(bytes).into_owned()))
}

fn opt_value(bytes: &Option<Vec<u8>>) -> Value {
    bytes.as_deref().map(value).unwrap_or(Value::Null)
}

fn status_name(status: InstanceStatus) -> &'static str {
    match status {
        InstanceStatus::Queued => "queued",
        InstanceStatus::Pending => "pending",
        InstanceStatus::Running => "running",
        InstanceStatus::Suspended => "suspended",
        InstanceStatus::Completed => "completed",
        InstanceStatus::Failed => "failed",
        InstanceStatus::Cancelled => "cancelled",
        InstanceStatus::NotStarted => "notStarted",
    }
}

fn status(name: &str) -> Result<InstanceStatus, Invalid> {
    Ok(match name {
        "queued" => InstanceStatus::Queued,
        "pending" => InstanceStatus::Pending,
        "running" => InstanceStatus::Running,
        "suspended" => InstanceStatus::Suspended,
        "completed" => InstanceStatus::Completed,
        "failed" => InstanceStatus::Failed,
        "cancelled" => InstanceStatus::Cancelled,
        "notStarted" => InstanceStatus::NotStarted,
        other => return Err(format!("unknown instance status `{other}`")),
    })
}

fn suspension_reason(reason: SuspensionReason) -> &'static str {
    match reason {
        SuspensionReason::Paused => "paused",
        SuspensionReason::WaitingSignal => "waitingSignal",
        SuspensionReason::WaitingInstances => "waitingInstances",
        SuspensionReason::Sleeping => "sleeping",
        SuspensionReason::Shutdown => "shutdown",
    }
}

/// `POST /v1/instances` body.
pub fn start_request(body: &Value) -> Result<StartRequest, Invalid> {
    let map = object(body, "a start request")?;
    let policy = match map.get("parentClosePolicy").and_then(Value::as_str) {
        Some("cancel") => ParentClosePolicy::Cancel,
        Some("leaveRunning") => ParentClosePolicy::LeaveRunning,
        _ => return Err("`parentClosePolicy` must be `cancel` or `leaveRunning`".into()),
    };
    Ok(StartRequest {
        workflow_id: string(map, "workflowId")?,
        version: opt_u32(map, "version")?,
        input: bytes(
            map.get("input")
                .unwrap_or(&json!({"data": {}, "variables": {}})),
        ),
        run_label: opt_string(map, "runLabel")?,
        parent_close_policy: policy,
    })
}

pub fn start_result(result: &StartResult) -> Value {
    json!({
        "instanceId": result.instance_id,
        "workflowId": result.workflow_id,
        "version": result.version,
        "runLabel": result.run_label,
        "replayed": result.replayed,
    })
}

pub fn summary(summary: &InstanceSummary) -> Value {
    json!({
        "instanceId": summary.instance_id,
        "workflowId": summary.workflow_id,
        "version": summary.version,
        "runLabel": summary.run_label,
        "parentInstanceId": summary.parent_instance_id,
        "status": status_name(summary.status),
        "suspensionReason": summary.suspension_reason.map(suspension_reason),
        "terminationReason": summary.termination_reason,
        "createdAtMs": summary.created_at_ms,
        "startedAtMs": summary.started_at_ms,
        "finishedAtMs": summary.finished_at_ms,
    })
}

fn terminal(terminal: &TerminalResult) -> Value {
    json!({
        "output": opt_value(&terminal.output),
        "outputBytes": terminal.output_bytes,
        "outputOmitted": terminal.output_omitted,
        "error": opt_value(&terminal.error),
        "errorOmitted": terminal.error_omitted,
    })
}

pub fn instance_detail(detail: &InstanceDetail) -> Value {
    json!({"instance": summary(&detail.instance), "terminal": terminal(&detail.terminal)})
}

pub fn state_read(read: &StateRead) -> Value {
    json!({
        "instance": summary(&read.instance),
        "state": opt_value(&read.state),
        "stateUpdatedAtMs": read.state_updated_at_ms,
    })
}

/// `POST /v1/instances/query` body.
pub fn query_request(body: &Value) -> Result<QueryRequest, Invalid> {
    let map = object(body, "a query")?;
    let statuses = match map.get("statuses") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .ok_or("statuses are strings".to_string())
                    .and_then(status)
            })
            .collect::<Result<_, _>>()?,
        Some(_) => return Err("`statuses` must be a list".into()),
    };
    let parent = match map.get("parent") {
        None | Some(Value::Null) => None,
        Some(Value::String(caller)) if caller == "caller" => Some(ParentFilter::Caller),
        Some(Value::Object(parent)) if parent.len() == 1 && parent.contains_key("instance") => {
            Some(ParentFilter::Instance(string(parent, "instance")?))
        }
        Some(_) => return Err("`parent` must be `\"caller\"` or `{\"instance\": id}`".into()),
    };
    let sort_by = match map.get("sortBy").and_then(Value::as_str) {
        None | Some("createdAt") => SortField::CreatedAt,
        Some("finishedAt") => SortField::FinishedAt,
        Some(other) => return Err(format!("unknown sort field `{other}`")),
    };
    let order = match map.get("order").and_then(Value::as_str) {
        None | Some("descending") => SortOrder::Descending,
        Some("ascending") => SortOrder::Ascending,
        Some(other) => return Err(format!("unknown sort order `{other}`")),
    };
    Ok(QueryRequest {
        workflow_id: opt_string(map, "workflowId")?,
        run_label: opt_string(map, "runLabel")?,
        statuses,
        parent,
        created_after_ms: opt_u64(map, "createdAfterMs")?,
        created_before_ms: opt_u64(map, "createdBeforeMs")?,
        finished_after_ms: opt_u64(map, "finishedAfterMs")?,
        finished_before_ms: opt_u64(map, "finishedBeforeMs")?,
        state: map.get("state").filter(|state| !state.is_null()).map(bytes),
        sort_by,
        order,
        page_size: opt_u32(map, "pageSize")?.unwrap_or(0),
        page_token: opt_string(map, "pageToken")?,
    })
}

pub fn instance_page(page: &InstancePage) -> Value {
    json!({
        "items": page.items.iter().map(summary).collect::<Vec<_>>(),
        "total": page.total,
        "nextPageToken": page.next_page_token,
    })
}

/// `POST /v1/instances/signals/pending` body.
pub fn pending_signals_request(body: &Value) -> Result<PendingSignalsRequest, Invalid> {
    let map = object(body, "a pending-signals request")?;
    let scope = match map.get("scope") {
        Some(Value::String(children)) if children == "children" => SignalScope::Children,
        Some(Value::Object(scope)) if scope.len() == 1 && scope.contains_key("instance") => {
            SignalScope::Instance(string(scope, "instance")?)
        }
        Some(Value::Object(scope)) if scope.len() == 1 && scope.contains_key("workflow") => {
            SignalScope::Workflow(string(scope, "workflow")?)
        }
        _ => {
            return Err(
                "`scope` must be `\"children\"`, `{\"instance\": id}` or `{\"workflow\": id}`"
                    .into(),
            );
        }
    };
    Ok(PendingSignalsRequest {
        scope,
        signal_id: opt_string(map, "signalId")?,
        action_key: opt_string(map, "actionKey")?,
        page_size: opt_u32(map, "pageSize")?.unwrap_or(0),
        page_token: opt_string(map, "pageToken")?,
    })
}

fn pending_signal(signal: &PendingSignal) -> Value {
    json!({
        "instanceId": signal.instance_id,
        "workflowId": signal.workflow_id,
        "signalId": signal.signal_id,
        "requestId": signal.request_id,
        "actionKey": signal.action_key,
        "responseSchema": opt_value(&signal.response_schema),
        "context": opt_value(&signal.context),
        "requestedAtMs": signal.requested_at_ms,
        "deadlineMs": signal.deadline_ms,
    })
}

pub fn pending_signal_page(page: &PendingSignalPage) -> Value {
    json!({
        "items": page.items.iter().map(pending_signal).collect::<Vec<_>>(),
        "nextPageToken": page.next_page_token,
    })
}

/// `POST /v1/instances/{id}/signals` body.
pub fn send_signal_request(instance_id: &str, body: &Value) -> Result<SendSignalRequest, Invalid> {
    let map = object(body, "a signal")?;
    Ok(SendSignalRequest {
        instance_id: instance_id.to_string(),
        signal_id: string(map, "signalId")?,
        action_key: opt_string(map, "actionKey")?,
        request_id: opt_string(map, "requestId")?,
        payload: bytes(map.get("payload").unwrap_or(&Value::Null)),
    })
}

pub fn send_signal_result(result: &SendSignalResult) -> Value {
    json!({"requestId": result.request_id, "replayed": result.replayed})
}

/// `POST /v1/instances/{id}/cancel` body (may be empty).
pub fn cancel_request(instance_id: &str, body: &Value) -> Result<CancelRequest, Invalid> {
    let empty = Map::new();
    let map = match body {
        Value::Null => &empty,
        other => object(other, "a cancel request")?,
    };
    Ok(CancelRequest {
        instance_id: instance_id.to_string(),
        reason: opt_string(map, "reason")?,
        grace_ms: opt_u64(map, "graceMs")?,
    })
}

pub fn command_result(result: &CommandResult) -> Value {
    json!({
        "instanceId": result.instance_id,
        "outcome": match result.outcome {
            CommandOutcome::Requested => "requested",
            CommandOutcome::Applied => "applied",
            CommandOutcome::Unchanged => "unchanged",
            CommandOutcome::AlreadyTerminal => "alreadyTerminal",
        },
        "replayed": result.replayed,
    })
}

/// The WIT name of a control error code (`not-child`, `requires-operation`, ...).
pub fn error_code(error: &ControlError) -> &'static str {
    match error.code {
        ControlErrorCode::Denied => "denied",
        ControlErrorCode::Invalid => "invalid",
        ControlErrorCode::NotFound => "not-found",
        ControlErrorCode::NotRunnable => "not-runnable",
        ControlErrorCode::NotChild => "not-child",
        ControlErrorCode::RequiresInstance => "requires-instance",
        ControlErrorCode::RequiresOperation => "requires-operation",
        ControlErrorCode::Capacity => "capacity",
        ControlErrorCode::ReplayConflict => "replay-conflict",
        ControlErrorCode::LabelConflict => "label-conflict",
        ControlErrorCode::TooLarge => "too-large",
        ControlErrorCode::Unavailable => "unavailable",
        ControlErrorCode::Unsupported => "unsupported",
        ControlErrorCode::NotWaiting => "not-waiting",
        ControlErrorCode::Ambiguous => "ambiguous",
        ControlErrorCode::AlreadyAnswered => "already-answered",
        ControlErrorCode::NotPausable => "not-pausable",
        ControlErrorCode::NotPaused => "not-paused",
        ControlErrorCode::Timeout => "timeout",
    }
}

/// Run `call` on `host` for `authority`, bounded by [`super::CALL_TIME_LIMIT`]
/// and audited like a WIT call. A body of the wrong shape is `invalid`.
pub async fn dispatch(
    host: &dyn ControlHost,
    authority: &ControlAuthority,
    call: ControlCall,
) -> Result<Value, ControlError> {
    let audit = CallAudit::start(authority, call.name());
    let deadline = Some(tokio::time::Instant::now() + super::CALL_TIME_LIMIT);
    let invalid = |message: Invalid| ControlError::new(ControlErrorCode::Invalid, message);
    let result = bounded(deadline, async {
        Ok(match call {
            ControlCall::Start(body) => start_result(
                &host
                    .start(authority, start_request(&body).map_err(invalid)?)
                    .await?,
            ),
            ControlCall::Get(id) => instance_detail(&host.get(authority, id).await?),
            ControlCall::GetState(id) => state_read(&host.get_state(authority, id).await?),
            ControlCall::Query(body) => instance_page(
                &host
                    .query(authority, query_request(&body).map_err(invalid)?)
                    .await?,
            ),
            ControlCall::PendingSignals(body) => pending_signal_page(
                &host
                    .list_pending_signals(
                        authority,
                        pending_signals_request(&body).map_err(invalid)?,
                    )
                    .await?,
            ),
            ControlCall::SendSignal(id, body) => send_signal_result(
                &host
                    .send_signal(authority, send_signal_request(&id, &body).map_err(invalid)?)
                    .await?,
            ),
            ControlCall::Cancel(id, body) => command_result(
                &host
                    .cancel(authority, cancel_request(&id, &body).map_err(invalid)?)
                    .await?,
            ),
            ControlCall::Pause(id) => command_result(&host.pause(authority, id).await?),
            ControlCall::Resume(id) => command_result(&host.resume(authority, id).await?),
        })
    })
    .await;
    audit.finish(&result);
    result
}

/// A control error as the workflow API's envelope: generic codes stay
/// generic, the rest are domain codes with the closest generic status.
pub fn api_error(error: &ControlError) -> ApiError {
    let generic = match error.code {
        ControlErrorCode::Denied => Some(ErrorCode::Denied),
        ControlErrorCode::Invalid => Some(ErrorCode::Invalid),
        ControlErrorCode::NotFound => Some(ErrorCode::NotFound),
        ControlErrorCode::Capacity => Some(ErrorCode::Capacity),
        ControlErrorCode::ReplayConflict => Some(ErrorCode::ReplayConflict),
        ControlErrorCode::TooLarge => Some(ErrorCode::TooLarge),
        ControlErrorCode::Unavailable => Some(ErrorCode::Unavailable),
        ControlErrorCode::Unsupported => Some(ErrorCode::Unsupported),
        ControlErrorCode::Timeout => Some(ErrorCode::Timeout),
        _ => None,
    };
    let mut api = match generic {
        Some(code) => ApiError::new(code, error.message.clone()),
        None => {
            let like = match error.code {
                ControlErrorCode::NotChild | ControlErrorCode::RequiresInstance => {
                    ErrorCode::Denied
                }
                ControlErrorCode::LabelConflict
                | ControlErrorCode::NotWaiting
                | ControlErrorCode::AlreadyAnswered
                | ControlErrorCode::NotPausable
                | ControlErrorCode::NotPaused => ErrorCode::ReplayConflict,
                _ => ErrorCode::Invalid,
            };
            ApiError::domain(error_code(error), like, error.message.clone())
        }
    };
    api.retry_after_ms = error.retry_after_ms;
    api
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_start_request_needs_its_policy_and_carries_json_input() {
        let request = start_request(&json!({
            "workflowId": "wf-1",
            "input": {"data": {"x": 1}, "variables": {}},
            "parentClosePolicy": "leaveRunning",
            "runLabel": "child-a"
        }))
        .unwrap();
        assert_eq!(request.workflow_id, "wf-1");
        assert_eq!(request.parent_close_policy, ParentClosePolicy::LeaveRunning);
        assert_eq!(
            serde_json::from_slice::<Value>(&request.input).unwrap(),
            json!({"data": {"x": 1}, "variables": {}})
        );
        assert!(start_request(&json!({"workflowId": "wf-1"})).is_err());
    }

    #[test]
    fn queries_and_scopes_parse() {
        let query = query_request(&json!({
            "statuses": ["running", "notStarted"],
            "parent": "caller",
            "sortBy": "finishedAt",
            "order": "ascending",
            "pageSize": 10
        }))
        .unwrap();
        assert_eq!(
            query.statuses,
            vec![InstanceStatus::Running, InstanceStatus::NotStarted]
        );
        assert_eq!(query.parent, Some(ParentFilter::Caller));
        assert_eq!(query.page_size, 10);
        assert!(query_request(&json!({"statuses": ["lost"]})).is_err());

        let scope = pending_signals_request(&json!({"scope": {"workflow": "wf-1"}})).unwrap();
        assert_eq!(scope.scope, SignalScope::Workflow("wf-1".into()));
        assert!(pending_signals_request(&json!({"scope": "everyone"})).is_err());
    }

    #[test]
    fn results_render_json_payloads_inline() {
        let detail = InstanceDetail {
            instance: InstanceSummary {
                instance_id: "i-1".into(),
                workflow_id: "wf-1".into(),
                version: Some(2),
                run_label: None,
                parent_instance_id: Some("p-1".into()),
                status: InstanceStatus::Completed,
                suspension_reason: None,
                termination_reason: None,
                created_at_ms: 1,
                started_at_ms: Some(2),
                finished_at_ms: Some(3),
            },
            terminal: TerminalResult {
                output: Some(br#"{"ok":true}"#.to_vec()),
                output_bytes: Some(11),
                output_omitted: false,
                error: None,
                error_omitted: false,
            },
        };
        let rendered = instance_detail(&detail);
        assert_eq!(rendered["instance"]["status"], "completed");
        assert_eq!(rendered["terminal"]["output"], json!({"ok": true}));
        assert_eq!(rendered["terminal"]["error"], Value::Null);
    }

    #[test]
    fn control_errors_keep_their_codes_on_the_api() {
        let error = api_error(&ControlError::new(ControlErrorCode::NotChild, "no"));
        assert_eq!(error.code.as_str(), "not-child");
        assert_eq!(error.http_status(), 403);
        let error = api_error(&ControlError::new(ControlErrorCode::NotFound, "gone"));
        assert_eq!(error.code, ErrorCode::NotFound);
        let error = api_error(&ControlError::new(
            ControlErrorCode::RequiresOperation,
            "op",
        ));
        assert_eq!(
            (error.code.as_str(), error.http_status()),
            ("requires-operation", 400)
        );
    }
}
