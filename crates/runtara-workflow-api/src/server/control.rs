//! The control family, `/v1/instances`, and trusted calls, `/v1/trusted`.
//! Handlers only parse; the backend enforces every rule (tenant scope,
//! children, ancestors, operations, artifact pins).

use axum::body::to_bytes;
use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use super::{Failure, TrustedCall, WorkflowApiState, binding_of};
use crate::{ApiError, ControlCall, ErrorCode, headers};

fn operation_of(request: &Request) -> Result<Option<String>, ApiError> {
    Ok(match request.headers().get(headers::OPERATION) {
        Some(value) => Some(
            value
                .to_str()
                .map_err(|_| ApiError::new(ErrorCode::Invalid, "Runtara-Operation is not ASCII"))?
                .to_string(),
        ),
        None => None,
    })
}

async fn call(
    state: WorkflowApiState,
    request: Request,
    build: impl FnOnce(serde_json::Value) -> ControlCall,
) -> Result<Response, Failure> {
    let binding = binding_of(&request)?;
    let operation = operation_of(&request)?;
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

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TrustedBody {
    connection_id: String,
    input: serde_json::Value,
}

pub(super) async fn post_trusted(
    State(state): State<WorkflowApiState>,
    Path((agent_id, capability_id)): Path<(String, String)>,
    request: Request,
) -> Result<Response, Failure> {
    let binding = binding_of(&request)?;
    let operation = operation_of(&request)?;
    let body = to_bytes(request.into_body(), state.caps.request_body_bytes)
        .await
        .map_err(|_| ApiError::new(ErrorCode::TooLarge, "trusted input is over the cap"))?;
    let body: TrustedBody = serde_json::from_slice(&body)
        .map_err(|e| ApiError::new(ErrorCode::Invalid, format!("trusted call: {e}")))?;
    let input = serde_json::to_vec(&body.input)
        .map_err(|e| ApiError::new(ErrorCode::Invalid, e.to_string()))?;
    let output = state
        .backend
        .trusted(
            &binding,
            operation.as_deref(),
            TrustedCall {
                agent_id,
                capability_id,
                connection_id: body.connection_id,
                input,
            },
        )
        .await?;
    let mut response = (StatusCode::OK, output).into_response();
    response.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/json"),
    );
    Ok(response)
}
