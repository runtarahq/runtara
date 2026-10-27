//! Child runs, `/v1/tasks`. Handlers only parse; the backend binds the child
//! to the launch's package and scope.

use axum::body::to_bytes;
use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use super::{Failure, WorkflowApiState, binding_of};
use crate::{ApiError, CancelAnswer, ErrorCode, TaskStart, TaskStarted, headers};

/// `Runtara-Task` metadata, raw body = the child's input.
pub(super) async fn post_task(
    State(state): State<WorkflowApiState>,
    request: Request,
) -> Result<Response, Failure> {
    let binding = binding_of(&request)?;
    let start: TaskStart = request
        .headers()
        .get(headers::TASK)
        .ok_or_else(|| ApiError::new(ErrorCode::Invalid, "Runtara-Task is required"))
        .and_then(|value| {
            serde_json::from_slice(value.as_bytes())
                .map_err(|e| ApiError::new(ErrorCode::Invalid, format!("Runtara-Task: {e}")))
        })?;
    let input = to_bytes(request.into_body(), state.caps.request_body_bytes)
        .await
        .map_err(|_| ApiError::new(ErrorCode::TooLarge, "task input is over the cap"))?;
    let started: TaskStarted = state
        .backend
        .start_task(&binding, start, input.to_vec())
        .await?;
    Ok((StatusCode::OK, axum::Json(started)).into_response())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Wait {
    #[serde(default)]
    wait_ms: u64,
}

/// `200` with the outcome once the task's execution was destroyed, `202`
/// while it runs.
pub(super) async fn get_task_outcome(
    State(state): State<WorkflowApiState>,
    Path(task_id): Path<String>,
    Query(wait): Query<Wait>,
    request: Request,
) -> Result<Response, Failure> {
    let binding = binding_of(&request)?;
    let Some(outcome) = state
        .backend
        .task_outcome(&binding, &task_id, wait.wait_ms)
        .await?
    else {
        return Ok(StatusCode::ACCEPTED.into_response());
    };
    let mut response = (StatusCode::OK, outcome.body()).into_response();
    response
        .headers_mut()
        .insert(headers::OUTCOME, HeaderValue::from_static(outcome.kind()));
    Ok(response)
}

pub(super) async fn post_task_cancel(
    State(state): State<WorkflowApiState>,
    Path(task_id): Path<String>,
    request: Request,
) -> Result<Response, Failure> {
    let binding = binding_of(&request)?;
    let status = state.backend.cancel_task(&binding, &task_id).await?;
    Ok((StatusCode::OK, axum::Json(CancelAnswer { status })).into_response())
}

pub(super) async fn delete_task(
    State(state): State<WorkflowApiState>,
    Path(task_id): Path<String>,
    request: Request,
) -> Result<Response, Failure> {
    let binding = binding_of(&request)?;
    state.backend.release_task(&binding, &task_id).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}
