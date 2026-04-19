//! Agent Execution Handler
//!
//! HTTP endpoint for executing agent capabilities on the host, on behalf of
//! workflow instances. This is the host-mediated I/O path for the WASM
//! transition (see docs/wasm-transition-plan.md, Step 1).
//!
//! Unlike the agent testing endpoint (which goes through the OCI dispatcher),
//! this endpoint calls `execute_capability` directly in the host process for
//! minimal overhead.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
};

use crate::api::dto::agent_execution::{
    ExecuteAgentErrorResponse, ExecuteAgentRequest, ExecuteAgentResponse,
};
use crate::api::services::agent_execution::{AgentExecutionError, AgentExecutionService};
use crate::middleware::tenant_auth::OrgId;

/// Execute an agent capability on the host
///
/// Workflow instances call this endpoint to delegate I/O-heavy agent work
/// (HTTP requests, database queries, SFTP operations, etc.) to the host process.
/// The host resolves connections, executes the agent, and returns the result.
///
/// Pure computation agents (transform, csv, xml, utils, text) can still run
/// in-process in the workflow binary — this endpoint is for agents that
/// require network access or platform-specific dependencies.
#[utoipa::path(
    post,
    path = "/api/runtime/agents/{agent_id}/capabilities/{capability_id}/execute",
    request_body = ExecuteAgentRequest,
    params(
        ("agent_id" = String, Path, description = "Agent module name (e.g., 'shopify', 'openai', 'http')"),
        ("capability_id" = String, Path, description = "Capability ID (e.g., 'get-products', 'http-request')")
    ),
    responses(
        (status = 200, description = "Agent executed (check 'success' field for result)", body = ExecuteAgentResponse),
        (status = 404, description = "Agent or capability not found", body = ExecuteAgentErrorResponse),
        (status = 500, description = "Internal error", body = ExecuteAgentErrorResponse)
    ),
    tag = "agents-controller"
)]
pub async fn execute_agent_handler(
    OrgId(tenant_id): OrgId,
    State(service): State<AgentExecutionService>,
    Path((agent_id, capability_id)): Path<(String, String)>,
    Json(request): Json<ExecuteAgentRequest>,
) -> Result<Json<ExecuteAgentResponse>, (StatusCode, Json<ExecuteAgentErrorResponse>)> {
    let result = service
        .execute(
            &tenant_id,
            &agent_id,
            &capability_id,
            request.inputs,
            request.connection_id.as_deref(),
        )
        .await;

    match result {
        Ok(execution_result) => Ok(Json(ExecuteAgentResponse {
            success: execution_result.success,
            output: execution_result.output,
            error: execution_result.error,
            execution_time_ms: execution_result.execution_time_ms,
        })),
        Err(err) => {
            let (status, error, message) = match err {
                AgentExecutionError::AgentNotFound(msg) => (
                    StatusCode::NOT_FOUND,
                    "Agent or capability not found".to_string(),
                    Some(msg),
                ),
                AgentExecutionError::ConnectionNotFound(msg) => (
                    StatusCode::NOT_FOUND,
                    "Connection not found".to_string(),
                    Some(msg),
                ),
                AgentExecutionError::ExecutionFailed(msg) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Execution failed".to_string(),
                    Some(msg),
                ),
                AgentExecutionError::DatabaseError(msg) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Database error".to_string(),
                    Some(msg),
                ),
            };

            Err((
                status,
                Json(ExecuteAgentErrorResponse {
                    success: false,
                    error,
                    message,
                }),
            ))
        }
    }
}
