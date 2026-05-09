//! Agents Service
//!
//! Business logic for querying agent metadata

use crate::api::dto::operators::{AgentInfo, AgentSummary, CapabilityInfo, get_agents};

#[derive(Debug)]
pub enum ServiceError {
    AgentNotFound,
    CapabilityNotFound,
}

impl std::fmt::Display for ServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServiceError::AgentNotFound => write!(f, "Agent not found"),
            ServiceError::CapabilityNotFound => write!(f, "Capability not found"),
        }
    }
}

pub struct AgentsService;

impl Default for AgentsService {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentsService {
    pub fn new() -> Self {
        Self
    }

    /// Get all agents (summary view)
    pub fn list_agents(&self) -> Vec<AgentSummary> {
        get_agents()
            .into_iter()
            .map(|agent| AgentSummary {
                id: agent.id,
                name: agent.name,
                description: agent.description,
                supports_connections: agent.supports_connections,
                integration_ids: agent.integration_ids,
            })
            .collect()
    }

    /// Get a specific agent by name
    pub fn get_agent(&self, name: &str) -> Result<AgentInfo, ServiceError> {
        get_agents()
            .into_iter()
            .find(|agent| agent.id.eq_ignore_ascii_case(name))
            .ok_or(ServiceError::AgentNotFound)
    }

    /// Get a specific capability within an agent
    pub fn get_capability(
        &self,
        agent_name: &str,
        capability_id: &str,
    ) -> Result<CapabilityInfo, ServiceError> {
        let agent = self.get_agent(agent_name)?;

        agent
            .capabilities
            .into_iter()
            .find(|cap| cap.id.eq_ignore_ascii_case(capability_id))
            .ok_or(ServiceError::CapabilityNotFound)
    }
}
