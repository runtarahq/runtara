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

    runtara_dsl::agent_meta::get_agents()
        .into_iter()
        .map(|mut agent| {
            if agent.id == "http" {
                agent.integration_ids = http_ids.clone();
            }
            agent
        })
        .collect()
}

/// Simplified agent info without capabilities (for list endpoint)
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct AgentSummary {
    pub id: String,
    pub name: String,
    pub description: String,
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

        // runtara-agents extractors (registered via inventory in runtara-agents)
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
}
