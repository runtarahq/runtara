//! Agents DTOs

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

// Re-export agent metadata types from runtara-dsl
pub use runtara_dsl::agent_meta::{AgentInfo, CapabilityInfo};

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
