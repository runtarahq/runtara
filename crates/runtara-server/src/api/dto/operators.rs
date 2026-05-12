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
}

/// Response for listing all agents
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ListAgentsResponse {
    pub agents: Vec<AgentSummary>,
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

    #[test]
    fn test_sharepoint_agent_is_registered() {
        let agents = get_agents();
        let sharepoint = agents
            .iter()
            .find(|a| a.id == "sharepoint")
            .expect("sharepoint agent should be registered");

        assert!(
            sharepoint
                .integration_ids
                .contains(&"microsoft_entra_client_credentials".to_string()),
            "sharepoint agent should reuse the microsoft_entra_client_credentials connection, got: {:?}",
            sharepoint.integration_ids
        );

        // Spot-check a few capabilities — we don't pin the full list to avoid
        // brittleness, but we do verify the read/write/copy pillars are wired.
        let cap_names: Vec<&str> = sharepoint
            .capabilities
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        for required in [
            "sharepoint_list_drives",
            "sharepoint_list_children",
            "sharepoint_download_file",
            "sharepoint_upload_file",
            "sharepoint_upload_file_large",
            "sharepoint_create_folder",
            "sharepoint_delete_item",
            "sharepoint_move_item",
            "sharepoint_copy_item",
            "sharepoint_get_copy_status",
            "sharepoint_search",
            "sharepoint_search_global",
        ] {
            assert!(
                cap_names.contains(&required),
                "sharepoint missing capability {required}; got: {:?}",
                cap_names
            );
        }
    }
}
