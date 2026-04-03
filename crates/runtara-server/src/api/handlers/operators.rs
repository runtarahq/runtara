//! Agents Handlers
//!
//! HTTP endpoints for querying agent metadata

use axum::{extract::Path, http::StatusCode, response::Json};

use crate::api::dto::operators::{AgentInfo, CapabilityInfo, ListAgentsResponse};
use crate::api::services::operators::{AgentsService, ServiceError};

/// Get all available agents (without capabilities details)
#[utoipa::path(
    get,
    path = "/api/runtime/agents",
    tag = "agents-controller",
    responses(
        (status = 200, description = "List of available agents", body = ListAgentsResponse),
    )
)]
pub async fn list_agents_handler() -> Result<Json<ListAgentsResponse>, StatusCode> {
    let service = AgentsService::new();
    let agents = service.list_agents();
    Ok(Json(ListAgentsResponse { agents }))
}

/// Get a specific agent by name
#[utoipa::path(
    get,
    path = "/api/runtime/agents/{name}",
    tag = "agents-controller",
    params(
        ("name" = String, Path, description = "Agent name (e.g., 'utils', 'transform', 'csv')")
    ),
    responses(
        (status = 200, description = "Agent information", body = serde_json::Value),
        (status = 404, description = "Agent not found"),
    )
)]
pub async fn get_agent_handler(Path(name): Path<String>) -> Result<Json<AgentInfo>, StatusCode> {
    let service = AgentsService::new();

    match service.get_agent(&name) {
        Ok(agent) => Ok(Json(agent)),
        Err(ServiceError::AgentNotFound) => Err(StatusCode::NOT_FOUND),
        Err(ServiceError::CapabilityNotFound) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// Get a specific capability within an agent
#[utoipa::path(
    get,
    path = "/api/runtime/agents/{name}/capabilities/{capability_id}",
    tag = "agents-controller",
    params(
        ("name" = String, Path, description = "Agent name (e.g., 'utils', 'transform', 'csv')"),
        ("capability_id" = String, Path, description = "Capability ID (e.g., 'random-double', 'extract')")
    ),
    responses(
        (status = 200, description = "Capability information", body = serde_json::Value),
        (status = 404, description = "Agent or capability not found"),
    )
)]
pub async fn get_capability_handler(
    Path((name, capability_id)): Path<(String, String)>,
) -> Result<Json<CapabilityInfo>, StatusCode> {
    let service = AgentsService::new();

    match service.get_capability(&name, &capability_id) {
        Ok(capability) => Ok(Json(capability)),
        Err(ServiceError::AgentNotFound) | Err(ServiceError::CapabilityNotFound) => {
            Err(StatusCode::NOT_FOUND)
        }
    }
}

/// Get connection schema for an agent (STUB)
#[utoipa::path(
    get,
    path = "/api/runtime/agents/{name}/connection-schema",
    tag = "agents-controller",
    params(
        ("name" = String, Path, description = "Agent name")
    ),
    responses(
        (status = 501, description = "Not implemented"),
    )
)]
pub async fn get_agent_connection_schema_handler(
    Path(name): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    let response = serde_json::json!({
        "success": false,
        "message": "This endpoint is not yet implemented",
        "endpoint": format!("/api/runtime/agents/{}/connection-schema", name),
        "status": 501
    });
    (StatusCode::NOT_IMPLEMENTED, Json(response))
}
