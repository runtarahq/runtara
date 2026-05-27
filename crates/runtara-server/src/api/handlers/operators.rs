//! Agents Handlers
//!
//! HTTP endpoints for querying agent metadata. The handlers read a single
//! shared `AgentsService` from `AppState` (built once at startup with the
//! embedded component dispatcher attached) so component-backed agents
//! consistently override the legacy registry on every call.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Json, Response},
};

use crate::api::dto::operators::{AgentInfo, AgentSummary, CapabilityInfo, ListAgentsResponse};
use crate::api::services::operators::{AgentsService, ServiceError};
use crate::entitlement_error::EntitlementDenial;

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
    // Filter against the tenant's materialised allowlist so discovery matches
    // invocation: an agent the per-handler gates would deny must not appear
    // in the listing the SPA and MCP `list_agents` tool render from.
    let allowlist = crate::config::entitlements().materialised_agents();
    let agents = filter_agents_by_allowlist(service.list_agents(), &allowlist);
    Ok(Json(ListAgentsResponse { agents }))
}

/// Drop agents that aren't in the tenant's materialised allowlist. Pure
/// function so the filtering rule is unit-testable without booting the
/// global config snapshot.
fn filter_agents_by_allowlist(
    agents: Vec<AgentSummary>,
    allowlist: &std::collections::BTreeSet<String>,
) -> Vec<AgentSummary> {
    agents
        .into_iter()
        .filter(|a| allowlist.contains(&a.id))
        .collect()
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
) -> Result<Json<AgentInfo>, Response> {
    // Per-agent allowlist check. Surfaces AGENT_NOT_ENABLED with the same
    // stable code the workflow compile gate emits.
    if let Err(err) = crate::config::entitlements().require_agent(&name) {
        return Err(EntitlementDenial::from(err).into_response());
    }
    match service.get_agent(&name) {
        Ok(agent) => Ok(Json(agent)),
        Err(ServiceError::AgentNotFound) => Err(StatusCode::NOT_FOUND.into_response()),
        Err(ServiceError::CapabilityNotFound) => {
            Err(StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
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
) -> Result<Json<CapabilityInfo>, Response> {
    // Per-agent allowlist check.
    if let Err(err) = crate::config::entitlements().require_agent(&name) {
        return Err(EntitlementDenial::from(err).into_response());
    }
    match service.get_capability(&name, &capability_id) {
        Ok(capability) => Ok(Json(capability)),
        Err(ServiceError::AgentNotFound) | Err(ServiceError::CapabilityNotFound) => {
            Err(StatusCode::NOT_FOUND.into_response())
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
pub async fn get_agent_connection_schema_handler(Path(name): Path<String>) -> Response {
    // Per-agent allowlist check.
    if let Err(err) = crate::config::entitlements().require_agent(&name) {
        return EntitlementDenial::from(err).into_response();
    }
    let response = serde_json::json!({
        "success": false,
        "message": "This endpoint is not yet implemented",
        "endpoint": format!("/api/runtime/agents/{}/connection-schema", name),
        "status": 501
    });
    (StatusCode::NOT_IMPLEMENTED, Json(response)).into_response()
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn summary(id: &str) -> AgentSummary {
        AgentSummary {
            id: id.to_string(),
            name: id.to_string(),
            description: String::new(),
            supports_connections: false,
            integration_ids: Vec::new(),
            component_backed: false,
        }
    }

    #[test]
    fn filter_drops_agents_outside_allowlist() {
        // Regression guard: discovery must mirror invocation. An agent the
        // per-handler gates would deny must not appear in `list_agents`.
        let allowlist: BTreeSet<String> = ["http", "csv"].into_iter().map(String::from).collect();
        let input = vec![summary("http"), summary("openai"), summary("csv")];
        let kept: Vec<String> = filter_agents_by_allowlist(input, &allowlist)
            .into_iter()
            .map(|a| a.id)
            .collect();
        assert_eq!(kept, vec!["http".to_string(), "csv".to_string()]);
    }

    #[test]
    fn filter_with_empty_allowlist_returns_empty() {
        // Explicit empty allowlist (`agents: []` in the entitlement JSON) is
        // the documented "deny everything" case. List must collapse to empty.
        let allowlist: BTreeSet<String> = BTreeSet::new();
        let input = vec![summary("http"), summary("csv")];
        assert!(filter_agents_by_allowlist(input, &allowlist).is_empty());
    }

    #[test]
    fn filter_with_full_allowlist_is_identity() {
        // The materialised allowlist contains every registered module under
        // the implicit-all default. The filter must be a no-op then —
        // otherwise the default-tier deployment would silently lose agents.
        let allowlist: BTreeSet<String> = ["http", "csv", "openai"]
            .into_iter()
            .map(String::from)
            .collect();
        let input = vec![summary("http"), summary("csv"), summary("openai")];
        let kept: Vec<String> = filter_agents_by_allowlist(input, &allowlist)
            .into_iter()
            .map(|a| a.id)
            .collect();
        assert_eq!(kept.len(), 3);
    }
}
