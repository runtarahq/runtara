//! The control family: `/v1/instances` (`runtara:control/api` of the old WIT
//! ABI). Handlers only parse; the backend's control service enforces every
//! rule (tenant scope, children, ancestors, operations).

use axum::body::to_bytes;
use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use super::{Failure, WorkflowApiState, binding_of};
use crate::{ApiError, ControlCall, ErrorCode, headers};

async fn call(
    state: WorkflowApiState,
    request: Request,
    build: impl FnOnce(serde_json::Value) -> ControlCall,
) -> Result<Response, Failure> {
    let binding = binding_of(&request)?;
    let operation = match request.headers().get(headers::OPERATION) {
        Some(value) => Some(
            value
                .to_str()
                .map_err(|_| ApiError::new(ErrorCode::Invalid, "Runtara-Operation is not ASCII"))?
                .to_string(),
        ),
        None => None,
    };
    let body = to_bytes(request.into_body(), state.caps.request_body_bytes)
        .await
        .map_err(|_| ApiError::new(ErrorCode::TooLarge, "control request is over the cap"))?;
    let body = if body.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&body)
            .map_err(|e| ApiError::new(ErrorCode::Invalid, format!("control request: {e}")))?
    };
    let result = state
        .backend
        .control(&binding, operation.as_deref(), build(body))
        .await?;
    Ok((StatusCode::OK, axum::Json(result)).into_response())
}

pub(super) async fn post_start(
    State(state): State<WorkflowApiState>,
    request: Request,
) -> Result<Response, Failure> {
    call(state, request, ControlCall::Start).await
}

pub(super) async fn get_instance(
    State(state): State<WorkflowApiState>,
    Path(id): Path<String>,
    request: Request,
) -> Result<Response, Failure> {
    call(state, request, |_| ControlCall::Get(id)).await
}

pub(super) async fn get_instance_state(
    State(state): State<WorkflowApiState>,
    Path(id): Path<String>,
    request: Request,
) -> Result<Response, Failure> {
    call(state, request, |_| ControlCall::GetState(id)).await
}

pub(super) async fn post_query(
    State(state): State<WorkflowApiState>,
    request: Request,
) -> Result<Response, Failure> {
    call(state, request, ControlCall::Query).await
}

pub(super) async fn post_pending_signals(
    State(state): State<WorkflowApiState>,
    request: Request,
) -> Result<Response, Failure> {
    call(state, request, ControlCall::PendingSignals).await
}

pub(super) async fn post_signal(
    State(state): State<WorkflowApiState>,
    Path(id): Path<String>,
    request: Request,
) -> Result<Response, Failure> {
    call(state, request, |body| ControlCall::SendSignal(id, body)).await
}

pub(super) async fn post_cancel(
    State(state): State<WorkflowApiState>,
    Path(id): Path<String>,
    request: Request,
) -> Result<Response, Failure> {
    call(state, request, |body| ControlCall::Cancel(id, body)).await
}

pub(super) async fn post_pause(
    State(state): State<WorkflowApiState>,
    Path(id): Path<String>,
    request: Request,
) -> Result<Response, Failure> {
    call(state, request, |_| ControlCall::Pause(id)).await
}

pub(super) async fn post_resume(
    State(state): State<WorkflowApiState>,
    Path(id): Path<String>,
    request: Request,
) -> Result<Response, Failure> {
    call(state, request, |_| ControlCall::Resume(id)).await
}
