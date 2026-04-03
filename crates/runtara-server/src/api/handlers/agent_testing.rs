//! Agent Testing Handler
//!
//! HTTP endpoints for testing agents using sandboxed container execution.
//! Agents are executed via the universal dispatcher in runtara-environment.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
};

use crate::api::dto::agent_testing::{TestAgentErrorResponse, TestAgentRequest, TestAgentResponse};
use crate::api::services::agent_testing::{AgentTestingService, ServiceError};
use crate::middleware::tenant_auth::OrgId;

/// Test an agent capability with given input
///
/// This endpoint allows testing agents in isolation using sandboxed container execution.
/// Agent testing must be enabled via ENABLE_OPERATOR_TESTING=true environment variable.
#[utoipa::path(
    post,
    path = "/api/runtime/agents/{name}/capabilities/{capability_id}/test",
    request_body = TestAgentRequest,
    params(
        ("name" = String, Path, description = "Agent name (e.g., 'utils', 'transform', 'csv')"),
        ("capability_id" = String, Path, description = "Capability ID (e.g., 'random-double', 'extract')")
    ),
    responses(
        (status = 200, description = "Agent executed successfully", body = TestAgentResponse),
        (status = 400, description = "Invalid input format", body = TestAgentErrorResponse),
        (status = 404, description = "Agent testing disabled or agent not found", body = TestAgentErrorResponse),
        (status = 429, description = "Rate limit exceeded", body = TestAgentErrorResponse),
        (status = 500, description = "Execution error", body = TestAgentErrorResponse),
        (status = 503, description = "Service unavailable", body = TestAgentErrorResponse)
    ),
    tag = "agents-controller"
)]
pub async fn test_agent_handler(
    OrgId(tenant_id): OrgId,
    State(service): State<Option<AgentTestingService>>,
    Path((agent_name, capability_id)): Path<(String, String)>,
    Json(request): Json<TestAgentRequest>,
) -> Result<Json<TestAgentResponse>, (StatusCode, Json<TestAgentErrorResponse>)> {
    // Check if service is configured
    let service = service.ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(TestAgentErrorResponse {
                success: false,
                error: "Agent testing is not enabled".to_string(),
                message: Some(
                    "Set ENABLE_OPERATOR_TESTING=true environment variable to enable this feature"
                        .to_string(),
                ),
            }),
        )
    })?;

    // Execute the agent test
    match service
        .test_agent(
            &tenant_id,
            &agent_name,
            &capability_id,
            request.input,
            request.connection_id,
        )
        .await
    {
        Ok(result) => Ok(Json(TestAgentResponse {
            success: result.success,
            output: result.output,
            error: result.error,
            execution_time_ms: result.execution_time_ms,
            max_memory_mb: result.max_memory_mb,
        })),
        Err(err) => {
            let (status, error, message) = match err {
                ServiceError::NotEnabled => (
                    StatusCode::NOT_FOUND,
                    "Agent testing is not enabled".to_string(),
                    Some("Set ENABLE_OPERATOR_TESTING=true to enable".to_string()),
                ),
                ServiceError::RateLimitExceeded(wait_time) => (
                    StatusCode::TOO_MANY_REQUESTS,
                    "Rate limit exceeded".to_string(),
                    Some(format!(
                        "Wait {:.2}s before retrying",
                        wait_time.as_secs_f64()
                    )),
                ),
                ServiceError::AgentNotFound(msg) => (
                    StatusCode::NOT_FOUND,
                    "Agent or capability not found".to_string(),
                    Some(msg),
                ),
                ServiceError::ConnectionNotFound(msg) => (
                    StatusCode::NOT_FOUND,
                    "Connection not found".to_string(),
                    Some(msg),
                ),
                ServiceError::ExecutionError(msg) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Execution failed".to_string(),
                    Some(msg),
                ),
                ServiceError::DatabaseError(msg) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Database error".to_string(),
                    Some(msg),
                ),
            };

            Err((
                status,
                Json(TestAgentErrorResponse {
                    success: false,
                    error,
                    message,
                }),
            ))
        }
    }
}
