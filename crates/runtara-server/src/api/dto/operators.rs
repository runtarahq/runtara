//! Agents DTOs

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

// Re-export agent metadata types from runtara-dsl
pub use runtara_dsl::agent_meta::{AgentInfo, CapabilityInfo};

/// Build agent list with HTTP agent's `integration_ids` dynamically expanded
/// to include every registered `HttpConnectionExtractor` (e.g. shopify, openai).
pub fn get_agents() -> Vec<AgentInfo> {
    let http_ids: Vec<String> = runtara_agents::extractors::get_http_extractor_ids()
        .into_iter()
        .map(String::from)
        .collect();

    runtara_agents::registry::get_agents()
        .into_iter()
        .map(|mut agent| {
            if agent.id == "http" {
                agent.integration_ids = http_ids.clone();
            }
            agent
        })
        .collect()
}

/// Simplified agent info without capabilities (for list endpoint).
///
/// Includes `integrationIds` and `supportsConnections` so callers (and MCP
/// agents) can identify which agents need a connection without an extra
/// `get_agent` round-trip per agent.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct AgentSummary {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(rename = "supportsConnections")]
    pub supports_connections: bool,
    /// Connection types this agent can use (e.g. "shopify_access_token",
    /// "openai_api_key"). Pass any of these to `list_connections` as the
    /// `integration_id` filter to find usable connections.
    #[serde(rename = "integrationIds")]
    pub integration_ids: Vec<String>,
    /// True when a WebAssembly component is loaded for this agent. Set by the
    /// server when `ComponentDispatcherService` is configured and knows about
    /// the agent. The capability surface itself is unchanged; this flag
    /// surfaces which execution backend a test_capability call would auto-pick
    /// (`engine=auto`).
    #[serde(rename = "componentBacked")]
    pub component_backed: bool,
}

/// Response for listing all agents
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ListAgentsResponse {
    pub agents: Vec<AgentSummary>,
}

/// Status of the embedded component dispatcher. Returned by
/// `GET /api/runtime/_internal/components/status`.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ComponentsStatusResponse {
    /// Whether a component dispatcher is configured at all
    /// (`RUNTARA_AGENT_COMPONENTS_DIR` set + at least one .wasm loaded).
    pub enabled: bool,
    /// Sorted list of agent ids the component dispatcher knows about.
    #[serde(rename = "loadedAgents")]
    pub loaded_agents: Vec<String>,
    /// Total capability count exposed by loaded components.
    #[serde(rename = "capabilityCount")]
    pub capability_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_http_agent_includes_all_extractors() {
        let agents = get_agents();
        let http = agents.iter().find(|a| a.id == "http").expect("http agent");

        // Base runtara extractors
        assert!(
            http.integration_ids.contains(&"http_bearer".to_string()),
            "missing http_bearer, got: {:?}",
            http.integration_ids
        );
        assert!(
            http.integration_ids.contains(&"http_api_key".to_string()),
            "missing http_api_key, got: {:?}",
            http.integration_ids
        );

        // runtara-agents extractors from the static registry
        assert!(
            http.integration_ids
                .contains(&"shopify_access_token".to_string()),
            "missing shopify_access_token, got: {:?}",
            http.integration_ids
        );
        assert!(
            http.integration_ids
                .contains(&"shopify_client_credentials".to_string()),
            "missing shopify_client_credentials, got: {:?}",
            http.integration_ids
        );
        assert!(
            http.integration_ids.contains(&"openai_api_key".to_string()),
            "missing openai_api_key, got: {:?}",
            http.integration_ids
        );
        assert!(
            http.integration_ids
                .contains(&"microsoft_entra_client_credentials".to_string()),
            "missing microsoft_entra_client_credentials, got: {:?}",
            http.integration_ids
        );
    }

    // NOTE: the standalone native `sharepoint` agent was deleted in
    // "agents: delete legacy native integration agents" — SharePoint now runs
    // as a WASM component, and its connection is surfaced through the `http`
    // agent's integration_ids (asserted in
    // `test_http_agent_includes_all_extractors` above). The former
    // `test_sharepoint_agent_is_registered`, which asserted a native agent in
    // `get_agents()`, was removed as stale.
}
