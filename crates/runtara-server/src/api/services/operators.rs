//! Agents Service
//!
//! Business logic for querying agent metadata. When a component dispatcher is
//! plugged in, agents loaded as WASM components are sourced from the
//! dispatcher's per-agent `AgentInfo` (parsed once at startup from each
//! component's sidecar `runtara_agent_<id>.meta.json`). Agents without a
//! loaded component fall back to the legacy static-registry-backed
//! `get_agents()` data.

use std::sync::Arc;

use runtara_component_host::ComponentDispatcherService;

use crate::api::dto::operators::{AgentInfo, AgentSummary, CapabilityInfo};

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

#[derive(Clone, Default)]
pub struct AgentsService {
    component_dispatcher: Option<Arc<ComponentDispatcherService>>,
    agent_catalog: Option<Arc<runtara_dsl::agent_meta::AgentCatalog>>,
}

impl AgentsService {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_component_dispatcher(
        mut self,
        dispatcher: Option<Arc<ComponentDispatcherService>>,
    ) -> Self {
        self.component_dispatcher = dispatcher;
        self
    }

    /// Attach the boot-time agent catalog. It is the authority on
    /// `integration_ids`: components whose integration list is host-resolved
    /// (the generic http agent) declare an empty one in their `meta.json`, and
    /// the catalog snapshot has already been augmented at boot. Without a
    /// catalog the service reports the raw declared ids — fine for tests, but
    /// the server always injects one.
    pub fn with_agent_catalog(
        mut self,
        catalog: Arc<runtara_dsl::agent_meta::AgentCatalog>,
    ) -> Self {
        self.agent_catalog = Some(catalog);
        self
    }

    /// The authoritative integration ids for an agent, per the catalog.
    fn integration_ids_of(&self, info: &AgentInfo) -> Vec<String> {
        match self.agent_catalog.as_deref() {
            Some(catalog) => catalog.integration_ids_for(&info.id),
            None => info.integration_ids.clone(),
        }
    }

    /// Build the canonical AgentInfo for a given id. Returns
    /// `(AgentInfo, component_backed)` — the bool is always `true`; it's kept
    /// for call-site compatibility with the DTO mapping.
    ///
    /// The component dispatcher is the *only* source of agents at runtime:
    /// everything executes as a WASM component, so an agent that isn't loaded
    /// by the dispatcher isn't runnable and must not be discoverable. An exact
    /// id match wins; a snake→kebab fold (`ai_tools` → `ai-tools`) is retried
    /// so legacy workflow JSON still resolves to the canonical component.
    fn agent_by_id(&self, name: &str) -> Option<(AgentInfo, bool)> {
        let d = self.component_dispatcher.as_deref()?;
        let kebab_fold = name.replace('_', "-");
        let info = d.agent_info_of(name).or_else(|| {
            if kebab_fold != name {
                d.agent_info_of(&kebab_fold)
            } else {
                None
            }
        })?;
        let mut info = info.clone();
        info.integration_ids = self.integration_ids_of(&info);
        Some((info, true))
    }

    /// Get all agents (summary view). Sourced purely from the component
    /// dispatcher — the runtime only dispatches WASM components, so the agent
    /// surface is exactly what's loaded. No dispatcher (or an empty one) means
    /// no agents; that's a misconfiguration the server logs loudly at boot
    /// rather than papering over with unrunnable static entries.
    pub fn list_agents(&self) -> Vec<AgentSummary> {
        let Some(d) = self.component_dispatcher.as_deref() else {
            return Vec::new();
        };

        let mut out: Vec<AgentSummary> = d
            .agent_ids()
            .filter_map(|agent_id| d.agent_info_of(agent_id))
            .map(|info| {
                let integration_ids = self.integration_ids_of(info);
                AgentSummary {
                    component_backed: true,
                    id: info.id.clone(),
                    name: info.name.clone(),
                    description: info.description.clone(),
                    supports_connections: info.supports_connections,
                    integration_ids,
                }
            })
            .collect();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    /// Get a specific agent by name (full info incl. capabilities).
    pub fn get_agent(&self, name: &str) -> Result<AgentInfo, ServiceError> {
        self.agent_by_id(name)
            .map(|(info, _)| info)
            .ok_or(ServiceError::AgentNotFound)
    }

    /// Get a specific capability within an agent.
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

    /// Snapshot of the component dispatcher state for
    /// `GET /api/runtime/_internal/components/status`.
    pub fn components_status(&self) -> (bool, Vec<String>, usize) {
        match self.component_dispatcher.as_deref() {
            None => (false, Vec::new(), 0),
            Some(d) => {
                let mut ids: Vec<String> = d.agent_ids().map(str::to_string).collect();
                ids.sort();
                let cap_count: usize = ids
                    .iter()
                    .map(|id| {
                        d.agent_info_of(id)
                            .map(|info| info.capabilities.len())
                            .unwrap_or(0)
                    })
                    .sum();
                (true, ids, cap_count)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(id: &str, integration_ids: &[&str]) -> AgentInfo {
        AgentInfo {
            id: id.to_string(),
            name: id.to_string(),
            description: String::new(),
            has_side_effects: true,
            supports_connections: true,
            integration_ids: integration_ids.iter().map(|s| s.to_string()).collect(),
            capabilities: vec![],
        }
    }

    /// The catalog is the authority, not the component's own declaration:
    /// agents whose integration list is host-resolved ship an empty one in
    /// their `meta.json`, and the boot catalog has already filled it in. This
    /// service must not re-derive that itself.
    #[test]
    fn catalog_integration_ids_win_over_the_declared_ones() {
        let catalog = Arc::new(runtara_dsl::agent_meta::AgentCatalog::from_agents(vec![
            agent("http", &["http_api_key", "http_bearer"]),
        ]));
        let service = AgentsService::new().with_agent_catalog(catalog);

        assert_eq!(
            service.integration_ids_of(&agent("http", &[])),
            vec!["http_api_key".to_string(), "http_bearer".to_string()]
        );
    }

    #[test]
    fn declared_integration_ids_are_used_without_a_catalog() {
        let service = AgentsService::new();

        assert_eq!(
            service.integration_ids_of(&agent("sftp", &["sftp"])),
            vec!["sftp".to_string()]
        );
    }
}
