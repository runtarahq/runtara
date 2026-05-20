//! Agents Handlers
//!
//! HTTP endpoints for querying agent metadata. The handlers read a single
//! shared `AgentsService` from `AppState` (built once at startup with the
//! embedded component dispatcher attached) so component-backed agents
//! consistently override the legacy registry on every call.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
};

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
pub async fn list_agents_handler(
    State(service): State<AgentsService>,
) -> Result<Json<ListAgentsResponse>, StatusCode> {
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
pub async fn get_agent_handler(
    State(service): State<AgentsService>,
    Path(name): Path<String>,
) -> Result<Json<AgentInfo>, StatusCode> {
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
    State(service): State<AgentsService>,
    Path((name, capability_id)): Path<(String, String)>,
) -> Result<Json<CapabilityInfo>, StatusCode> {
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

/// Snapshot of the embedded component dispatcher.
#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct ComponentsStatusResponse {
    pub loaded: bool,
    pub agent_ids: Vec<String>,
    pub capability_count: usize,
}

/// Get a snapshot of the embedded component dispatcher's loaded agents and
/// total declared-capability count. Used by ops and the components A/B
/// switching logic.
#[utoipa::path(
    get,
    path = "/api/runtime/_internal/components/status",
    tag = "agents-controller",
    responses(
        (status = 200, description = "Component dispatcher status", body = ComponentsStatusResponse),
    )
)]
pub async fn components_status_handler(
    State(service): State<AgentsService>,
) -> Json<ComponentsStatusResponse> {
    let (loaded, agent_ids, capability_count) = service.components_status();
    Json(ComponentsStatusResponse {
        loaded,
        agent_ids,
        capability_count,
    })
}
