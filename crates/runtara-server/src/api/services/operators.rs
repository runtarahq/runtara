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

#[derive(Clone, Default)]
pub struct AgentsService {
    component_dispatcher: Option<Arc<ComponentDispatcherService>>,
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

    /// Build the canonical AgentInfo for a given id. Returns
    /// `(AgentInfo, component_backed)`. The component-backed path is a
    /// straight clone of the parsed sidecar JSON; the legacy path uses the
    /// static registry built into the server binary.
    ///
    /// Resolution order — when the component dispatcher is loaded, an exact
    /// match wins; otherwise a snake-to-kebab fold (`ai_tools` →
    /// `ai-tools`) is retried so old workflow JSON still resolves to the
    /// canonical component agent. Only when no component matches do we fall
    /// back to the static registry — and even then we hide the static entry
    /// if a kebab-equivalent component exists, so callers never see a legacy
    /// duplicate of a deployed component.
    fn agent_by_id(&self, name: &str) -> Option<(AgentInfo, bool)> {
        if let Some(d) = self.component_dispatcher.as_deref() {
            let exact = d.agent_info_of(name);
            let kebab_fold = name.replace('_', "-");
            let folded = if kebab_fold != name {
                d.agent_info_of(&kebab_fold)
            } else {
                None
            };
            if let Some(info) = exact.or(folded) {
                let mut info = info.clone();
                // The http agent's integration list is dynamic (any registered
                // HttpConnectionExtractor counts). Preserve that augmentation
                // even when http is component-backed.
                if info.id == "http" {
                    info.integration_ids = http_integration_ids();
                }
                return Some((info, true));
            }
            // No component matches — fall through to static, but suppress
            // names whose kebab form IS deployed as a component (the static
            // shadow exists only for binary-internal use, not the API).
            if d.agent_info_of(&kebab_fold).is_some() {
                return None;
            }
        }
        get_agents()
            .into_iter()
            .find(|a| a.id.eq_ignore_ascii_case(name))
            .map(|a| (a, false))
    }

    /// Get all agents (summary view). When the component dispatcher is
    /// loaded, the kebab-cased component agents are the canonical surface and
    /// any static-registered snake_case shadows (e.g. `ai_tools` next to
    /// `ai-tools`) are dropped so discovery doesn't list every agent twice.
    /// Static-only agents (no kebab equivalent in the dispatcher) still pass
    /// through. With no dispatcher loaded — CLI / minimal deployments — the
    /// static registry is returned as-is.
    pub fn list_agents(&self) -> Vec<AgentSummary> {
        use std::collections::{BTreeMap, BTreeSet};

        // Canonical kebab ids deployed via the component dispatcher.
        let canonical_kebab: BTreeSet<String> = self
            .component_dispatcher
            .as_deref()
            .map(|d| d.agent_ids().map(String::from).collect())
            .unwrap_or_default();

        let mut by_id: BTreeMap<String, (AgentInfo, bool)> = BTreeMap::new();

        for agent in get_agents() {
            // Skip the legacy static entry when the dispatcher already exposes
            // an equivalent under its kebab id — `ai_tools` is hidden in favor
            // of `ai-tools`, and so on.
            if canonical_kebab.contains(&agent.id.replace('_', "-")) {
                continue;
            }
            by_id.insert(agent.id.clone(), (agent, false));
        }

        if let Some(d) = self.component_dispatcher.as_deref() {
            for agent_id in d.agent_ids() {
                if let Some(info) = d.agent_info_of(agent_id) {
                    let mut info = info.clone();
                    if info.id == "http" {
                        info.integration_ids = http_integration_ids();
                    }
                    by_id.insert(agent_id.to_string(), (info, true));
                }
            }
        }

        by_id
            .into_values()
            .map(|(agent, component_backed)| AgentSummary {
                component_backed,
                id: agent.id,
                name: agent.name,
                description: agent.description,
                supports_connections: agent.supports_connections,
                integration_ids: agent.integration_ids,
            })
            .collect()
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

fn http_integration_ids() -> Vec<String> {
    runtara_agents::extractors::get_http_extractor_ids()
        .into_iter()
        .map(String::from)
        .collect()
}
