//! Run services: checkpoints, lifecycle and custom signals, time, heartbeat
//! and events (the `runtara:workflow/runtime` family of the old WIT ABI).

use std::time::Duration;

use axum::body::to_bytes;
use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use serde::{Deserialize, Serialize};

use super::{Failure, WorkflowApiState, binding_of};
use crate::{ApiError, CustomSignalNotice, ErrorCode, Event, LifecycleNotice, headers};

/// A pending lifecycle command (cancel, pause, resume, shutdown).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LifecycleSignal {
    pub signal_type: String,
    pub command_id: String,
    pub payload: Vec<u8>,
    pub checkpoint_id: Option<String>,
}

/// The result of writing a checkpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckpointWrite {
    /// False when the checkpoint already existed: `state` is the one that won.
    pub stored: bool,
    pub state: Vec<u8>,
    pub pending_signal: Option<LifecycleSignal>,
    pub custom_signal: Option<CustomSignalNotice>,
}

impl From<&LifecycleSignal> for LifecycleNotice {
    fn from(signal: &LifecycleSignal) -> Self {
        Self {
            signal_type: signal.signal_type.clone(),
            command_id: signal.command_id.clone(),
            checkpoint_id: signal.checkpoint_id.clone(),
        }
    }
}

fn json_header<T: Serialize>(value: &T) -> Result<HeaderValue, ApiError> {
    let json = serde_json::to_string(value)
        .map_err(|e| ApiError::new(ErrorCode::Unavailable, e.to_string()))?;
    HeaderValue::from_str(&json)
        .map_err(|_| ApiError::new(ErrorCode::Unavailable, "announcement is not a header value"))
}

fn octets(body: Vec<u8>, status: StatusCode) -> Response {
    let mut response = (status, Bytes::from(body)).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    response
}

fn json<T: Serialize>(value: &T) -> Result<Response, ApiError> {
    let body = serde_json::to_vec(value)
        .map_err(|e| ApiError::new(ErrorCode::Unavailable, e.to_string()))?;
    let mut response = body.into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    Ok(response)
}

pub(super) async fn get_checkpoint(
    State(state): State<WorkflowApiState>,
    Path(id): Path<String>,
    request: Request,
) -> Result<Response, Failure> {
    let binding = binding_of(&request)?;
    match state.backend.get_checkpoint(&binding, &id).await? {
        Some(body) => Ok(octets(body, StatusCode::OK)),
        None => Err(ApiError::new(ErrorCode::NotFound, "no such checkpoint").into()),
    }
}

pub(super) async fn put_checkpoint(
    State(state): State<WorkflowApiState>,
    Path(id): Path<String>,
    request: Request,
) -> Result<Response, Failure> {
    let binding = binding_of(&request)?;
    let body = to_bytes(request.into_body(), state.caps.checkpoint_state_bytes)
        .await
        .map_err(|_| {
            ApiError::new(
                ErrorCode::TooLarge,
                format!(
                    "checkpoint state is over {} bytes",
                    state.caps.checkpoint_state_bytes
                ),
            )
        })?;
    if body.is_empty() {
        return Err(ApiError::new(
            ErrorCode::Invalid,
            "checkpoint state is empty; read a checkpoint with GET",
        )
        .into());
    }
    let write = state
        .backend
        .put_checkpoint(&binding, &id, body.to_vec())
        .await?;
    let status = if write.stored {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    let mut response = octets(write.state, status);
    if let Some(signal) = &write.pending_signal {
        response.headers_mut().insert(
            headers::PENDING_SIGNAL,
            json_header(&LifecycleNotice::from(signal))?,
        );
    }
    if let Some(signal) = &write.custom_signal {
        response
            .headers_mut()
            .insert(headers::CUSTOM_SIGNAL, json_header(signal)?);
    }
    Ok(response)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RetryAttempt {
    attempt_number: u32,
    #[serde(default)]
    error_message: Option<String>,
}

pub(super) async fn post_retry_attempt(
    State(state): State<WorkflowApiState>,
    Path(id): Path<String>,
    request: Request,
) -> Result<Response, Failure> {
    let binding = binding_of(&request)?;
    let body = to_bytes(request.into_body(), state.caps.request_body_bytes)
        .await
        .map_err(|_| ApiError::new(ErrorCode::TooLarge, "retry attempt is over the cap"))?;
    let attempt: RetryAttempt = serde_json::from_slice(&body)
        .map_err(|e| ApiError::new(ErrorCode::Invalid, format!("retry attempt: {e}")))?;
    state
        .backend
        .record_retry_attempt(&binding, &id, attempt.attempt_number, attempt.error_message)
        .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub(super) async fn get_lifecycle_signal(
    State(state): State<WorkflowApiState>,
    request: Request,
) -> Result<Response, Failure> {
    let binding = binding_of(&request)?;
    match state.backend.poll_lifecycle_signal(&binding).await? {
        None => Ok(StatusCode::NO_CONTENT.into_response()),
        Some(signal) => {
            let notice = json_header(&LifecycleNotice::from(&signal))?;
            let mut response = octets(signal.payload, StatusCode::OK);
            response.headers_mut().insert(headers::SIGNAL, notice);
            Ok(response)
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Ack {
    signal_type: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Acked {
    applied: bool,
}

pub(super) async fn ack_lifecycle_signal(
    State(state): State<WorkflowApiState>,
    Path(command_id): Path<String>,
    request: Request,
) -> Result<Response, Failure> {
    let binding = binding_of(&request)?;
    let body = to_bytes(request.into_body(), state.caps.request_body_bytes)
        .await
        .map_err(|_| ApiError::new(ErrorCode::TooLarge, "acknowledgement is over the cap"))?;
    let ack: Ack = serde_json::from_slice(&body)
        .map_err(|e| ApiError::new(ErrorCode::Invalid, format!("acknowledgement: {e}")))?;
    let applied = state
        .backend
        .ack_lifecycle_signal(&binding, &command_id, &ack.signal_type)
        .await?;
    Ok(json(&Acked { applied })?)
}

pub(super) async fn check_lifecycle_signals(
    State(state): State<WorkflowApiState>,
    request: Request,
) -> Result<Response, Failure> {
    let binding = binding_of(&request)?;
    let stopped = state.backend.check_lifecycle_signals(&binding).await?;
    Ok(json(&serde_json::json!({"stopped": stopped}))?)
}

pub(super) async fn lifecycle_cancelled(
    State(state): State<WorkflowApiState>,
    request: Request,
) -> Result<Response, Failure> {
    let binding = binding_of(&request)?;
    let cancelled = state.backend.lifecycle_cancelled(&binding).await?;
    Ok(json(&serde_json::json!({"cancelled": cancelled}))?)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Wait {
    #[serde(default)]
    wait_ms: u64,
}

/// How often a long poll re-asks the backend.
const LONG_POLL_STEP: Duration = Duration::from_millis(200);

pub(super) async fn get_custom_signal(
    State(state): State<WorkflowApiState>,
    Path(checkpoint_id): Path<String>,
    Query(wait): Query<Wait>,
    request: Request,
) -> Result<Response, Failure> {
    let binding = binding_of(&request)?;
    let wait_ms = wait.wait_ms.min(state.caps.long_poll_ms);
    let deadline = tokio::time::Instant::now() + Duration::from_millis(wait_ms);
    loop {
        if let Some(payload) = state
            .backend
            .poll_custom_signal(&binding, &checkpoint_id)
            .await?
        {
            return Ok(octets(payload, StatusCode::OK));
        }
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return Ok(StatusCode::ACCEPTED.into_response());
        }
        tokio::time::sleep((deadline - now).min(LONG_POLL_STEP)).await;
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Now {
    now_ms: u64,
}

pub(super) async fn get_time(
    State(state): State<WorkflowApiState>,
    request: Request,
) -> Result<Response, Failure> {
    let binding = binding_of(&request)?;
    let now_ms = state.backend.now_ms(&binding).await?;
    Ok(json(&Now { now_ms })?)
}

pub(super) async fn post_heartbeat(
    State(state): State<WorkflowApiState>,
    request: Request,
) -> Result<Response, Failure> {
    let binding = binding_of(&request)?;
    state.backend.heartbeat(&binding).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub(super) async fn post_events(
    State(state): State<WorkflowApiState>,
    request: Request,
) -> Result<Response, Failure> {
    let binding = binding_of(&request)?;
    let body = to_bytes(request.into_body(), state.caps.event_batch_bytes)
        .await
        .map_err(|_| {
            ApiError::new(
                ErrorCode::TooLarge,
                format!("event batch is over {} bytes", state.caps.event_batch_bytes),
            )
        })?;
    let text = std::str::from_utf8(&body)
        .map_err(|_| ApiError::new(ErrorCode::Invalid, "event batch is not UTF-8"))?;
    let mut events = Vec::new();
    for (n, line) in text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .enumerate()
    {
        if n >= state.caps.event_batch_lines {
            return Err(ApiError::new(
                ErrorCode::TooLarge,
                format!(
                    "an event batch holds at most {} events",
                    state.caps.event_batch_lines
                ),
            )
            .into());
        }
        let event: Event = serde_json::from_str(line)
            .map_err(|e| ApiError::new(ErrorCode::Invalid, format!("event {n}: {e}")))?;
        if event.kind.is_empty() {
            return Err(ApiError::new(ErrorCode::Invalid, format!("event {n} has no kind")).into());
        }
        events.push(event);
    }
    state.backend.append_events(&binding, events).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub(super) async fn patch_state(
    State(state): State<WorkflowApiState>,
    Path(key): Path<String>,
    request: Request,
) -> Result<Response, Failure> {
    let binding = binding_of(&request)?;
    let body = to_bytes(request.into_body(), state.caps.request_body_bytes)
        .await
        .map_err(|_| ApiError::new(ErrorCode::TooLarge, "state patch is over the cap"))?;
    let patch = match serde_json::from_slice::<serde_json::Value>(&body) {
        Ok(serde_json::Value::Object(patch)) => patch,
        Ok(_) => {
            return Err(
                ApiError::new(ErrorCode::Invalid, "a state patch must be a JSON object").into(),
            );
        }
        Err(e) => {
            return Err(ApiError::new(ErrorCode::Invalid, format!("state patch: {e}")).into());
        }
    };
    state.backend.set_state(&binding, &key, patch).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub(super) async fn get_state(
    State(state): State<WorkflowApiState>,
    Path(key): Path<String>,
    request: Request,
) -> Result<Response, Failure> {
    let binding = binding_of(&request)?;
    let current = state.backend.get_state(&binding, &key).await?;
    Ok(json(&serde_json::Value::Object(current))?)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InputRegistration {
    descriptor: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    deadline_ms: Option<u64>,
}

pub(super) async fn put_input(
    State(state): State<WorkflowApiState>,
    Path(signal_id): Path<String>,
    request: Request,
) -> Result<Response, Failure> {
    let binding = binding_of(&request)?;
    let body = to_bytes(request.into_body(), state.caps.request_body_bytes)
        .await
        .map_err(|_| ApiError::new(ErrorCode::TooLarge, "input registration is over the cap"))?;
    let registration: InputRegistration = serde_json::from_slice(&body)
        .map_err(|e| ApiError::new(ErrorCode::Invalid, format!("input registration: {e}")))?;
    if registration
        .descriptor
        .get("signal_id")
        .and_then(serde_json::Value::as_str)
        != Some(signal_id.as_str())
    {
        return Err(ApiError::new(
            ErrorCode::Invalid,
            "the descriptor's signal_id must be the input in the path",
        )
        .into());
    }
    let descriptor = serde_json::to_vec(&registration.descriptor)
        .map_err(|e| ApiError::new(ErrorCode::Invalid, e.to_string()))?;
    state
        .backend
        .register_input(&binding, descriptor, registration.deadline_ms)
        .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub(super) async fn get_input(
    State(state): State<WorkflowApiState>,
    Path(signal_id): Path<String>,
    Query(wait): Query<Wait>,
    request: Request,
) -> Result<Response, Failure> {
    let binding = binding_of(&request)?;
    let wait_ms = wait.wait_ms.min(state.caps.long_poll_ms);
    let deadline = tokio::time::Instant::now() + Duration::from_millis(wait_ms);
    loop {
        let read = state.backend.poll_input(&binding, &signal_id).await?;
        let now = tokio::time::Instant::now();
        if read != crate::InputRead::Open || now >= deadline {
            let status = if read == crate::InputRead::Open && wait_ms > 0 {
                StatusCode::ACCEPTED
            } else {
                StatusCode::OK
            };
            let mut response = json(&read)?;
            *response.status_mut() = status;
            return Ok(response);
        }
        tokio::time::sleep((deadline - now).min(LONG_POLL_STEP)).await;
    }
}

pub(super) async fn delete_input(
    State(state): State<WorkflowApiState>,
    Path(signal_id): Path<String>,
    request: Request,
) -> Result<Response, Failure> {
    let binding = binding_of(&request)?;
    let read = state.backend.close_input(&binding, &signal_id).await?;
    Ok(json(&read)?)
}

pub(super) async fn put_wait(
    State(state): State<WorkflowApiState>,
    Path(key): Path<String>,
    request: Request,
) -> Result<Response, Failure> {
    let binding = binding_of(&request)?;
    let body = to_bytes(request.into_body(), state.caps.request_body_bytes)
        .await
        .map_err(|_| ApiError::new(ErrorCode::TooLarge, "wait request is over the cap"))?;
    let wait: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|e| ApiError::new(ErrorCode::Invalid, format!("wait request: {e}")))?;
    if !wait.is_object() {
        return Err(
            ApiError::new(ErrorCode::Invalid, "a wait request must be a JSON object").into(),
        );
    }
    let read = state.backend.register_wait(&binding, &key, wait).await?;
    Ok(json(&read)?)
}

/// Whether a wait read is still pending (its `resolution` is null).
fn wait_pending(read: &serde_json::Value) -> bool {
    read.get("resolution")
        .is_none_or(serde_json::Value::is_null)
}

pub(super) async fn get_wait(
    State(state): State<WorkflowApiState>,
    Path(key): Path<String>,
    Query(wait): Query<Wait>,
    request: Request,
) -> Result<Response, Failure> {
    let binding = binding_of(&request)?;
    let wait_ms = wait.wait_ms.min(state.caps.long_poll_ms);
    let deadline = tokio::time::Instant::now() + Duration::from_millis(wait_ms);
    loop {
        let read = state.backend.poll_wait(&binding, &key).await?;
        let now = tokio::time::Instant::now();
        if !wait_pending(&read) || now >= deadline {
            let mut response = json(&read)?;
            if wait_pending(&read) && wait_ms > 0 {
                *response.status_mut() = StatusCode::ACCEPTED;
            }
            return Ok(response);
        }
        tokio::time::sleep((deadline - now).min(LONG_POLL_STEP)).await;
    }
}

pub(super) async fn delete_wait(
    State(state): State<WorkflowApiState>,
    Path(key): Path<String>,
    request: Request,
) -> Result<Response, Failure> {
    let binding = binding_of(&request)?;
    state.backend.release_wait(&binding, &key).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EnterOperation {
    checkpoint_key: String,
    attempt: u32,
    #[serde(default)]
    load: bool,
}

pub(super) async fn post_operation(
    State(state): State<WorkflowApiState>,
    request: Request,
) -> Result<Response, Failure> {
    let binding = binding_of(&request)?;
    let body = to_bytes(request.into_body(), state.caps.request_body_bytes)
        .await
        .map_err(|_| ApiError::new(ErrorCode::TooLarge, "operation request is over the cap"))?;
    let enter: EnterOperation = serde_json::from_slice(&body)
        .map_err(|e| ApiError::new(ErrorCode::Invalid, format!("operation request: {e}")))?;
    let (operation_id, continuation) = state
        .backend
        .enter_operation(&binding, &enter.checkpoint_key, enter.attempt, enter.load)
        .await?;
    let mut response = match continuation {
        Some(continuation) => octets(continuation, StatusCode::OK),
        None => StatusCode::NO_CONTENT.into_response(),
    };
    response.headers_mut().insert(
        headers::OPERATION,
        HeaderValue::from_str(&operation_id)
            .map_err(|_| ApiError::new(ErrorCode::Unavailable, "bad operation id"))?,
    );
    Ok(response)
}

pub(super) async fn post_operation_suspend(
    State(state): State<WorkflowApiState>,
    Path(operation_id): Path<String>,
    request: Request,
) -> Result<Response, Failure> {
    let binding = binding_of(&request)?;
    let wakes: Vec<crate::Wake> = request
        .headers()
        .get(headers::WAKES)
        .ok_or_else(|| ApiError::new(ErrorCode::Invalid, "Runtara-Wakes is required"))
        .and_then(|value| {
            serde_json::from_slice(value.as_bytes())
                .map_err(|e| ApiError::new(ErrorCode::Invalid, format!("Runtara-Wakes: {e}")))
        })?;
    let body = to_bytes(request.into_body(), state.caps.continuation_bytes)
        .await
        .map_err(|_| {
            ApiError::new(
                ErrorCode::TooLarge,
                format!(
                    "a continuation is at most {} bytes",
                    state.caps.continuation_bytes
                ),
            )
        })?;
    state
        .backend
        .suspend_operation(&binding, &operation_id, body.to_vec(), wakes)
        .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExitOperation {
    failed: bool,
}

pub(super) async fn post_operation_exit(
    State(state): State<WorkflowApiState>,
    Path(operation_id): Path<String>,
    request: Request,
) -> Result<Response, Failure> {
    let binding = binding_of(&request)?;
    let body = to_bytes(request.into_body(), state.caps.request_body_bytes)
        .await
        .map_err(|_| ApiError::new(ErrorCode::TooLarge, "exit request is over the cap"))?;
    let exit: ExitOperation = serde_json::from_slice(&body)
        .map_err(|e| ApiError::new(ErrorCode::Invalid, format!("exit request: {e}")))?;
    state
        .backend
        .exit_operation(&binding, &operation_id, exit.failed)
        .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub(super) async fn delete_operation(
    State(state): State<WorkflowApiState>,
    Path(operation_id): Path<String>,
    request: Request,
) -> Result<Response, Failure> {
    let binding = binding_of(&request)?;
    state
        .backend
        .release_operation(&binding, &operation_id)
        .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}
