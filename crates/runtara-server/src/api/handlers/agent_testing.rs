//! Agent Testing Handler
//!
//! HTTP endpoints for testing agents. Routing between the embedded wasmtime
//! component path and the legacy dispatcher image is decided by the
//! `?engine=auto|components|legacy` query parameter.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Json, Response},
};

use crate::api::dto::agent_testing::{
    TestAgentErrorResponse, TestAgentQuery, TestAgentRequest, TestAgentResponse,
};
use crate::api::services::agent_testing::{AgentTestingService, ServiceError};
use crate::entitlement_error::EntitlementDenial;
use crate::middleware::tenant_auth::OrgId;

/// Test an agent capability with given input
///
/// This endpoint allows testing agents in isolation. Pass
/// `?engine=components` to force the embedded wasmtime path or
/// `?engine=legacy` to force the dispatcher image; the default `auto`
/// routes to components when a WASM component is loaded for the agent.
#[utoipa::path(
    post,
    path = "/api/runtime/agents/{name}/capabilities/{capability_id}/test",
    request_body = TestAgentRequest,
    params(
        ("name" = String, Path, description = "Agent name (e.g., 'utils', 'transform', 'csv')"),
        ("capability_id" = String, Path, description = "Capability ID (e.g., 'random-double', 'extract')"),
        ("engine" = Option<String>, Query, description = "Engine: auto | components | legacy (default: auto)"),
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
    Query(query): Query<TestAgentQuery>,
    Json(request): Json<TestAgentRequest>,
) -> Result<Json<TestAgentResponse>, Response> {
    // Per-agent allowlist check.
    if let Err(err) = crate::config::entitlements().require_agent(&agent_name) {
        return Err(EntitlementDenial::from(err).into_response());
    }

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
            .into_response()
    })?;

    match service
        .test_agent(
            &tenant_id,
            &agent_name,
            &capability_id,
            request.input,
            request.connection_id,
            query.engine,
        )
        .await
    {
        Ok(result) => Ok(Json(TestAgentResponse {
            success: result.success,
            output: result.output,
            error: result.error,
            execution_time_ms: result.execution_time_ms,
            max_memory_mb: result.max_memory_mb,
            engine: Some(result.engine),
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
            )
                .into_response())
        }
    }
}
