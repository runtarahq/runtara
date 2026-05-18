//! `ComponentDispatcherService` — the host-facing API that
//! `AgentTestingService` calls into instead of dispatcher-image roundtrips.
//!
//! Loads `runtara_agent_*.wasm` components from a directory at construction
//! time, pre-instantiates each, and exposes per-capability test invocation.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use wasmtime::Engine;

use crate::bindings::exports::runtara::agent::capabilities::{CapabilityInfo, ConnectionInfo};
use crate::engine::{EngineConfig, build_engine};
use crate::host_state::{CallContext, HostState};
use crate::registry::{LoadedAgent, build_linker, instantiate, load_agent};

/// Server-facing per-call request shape. Mirrors today's `TestAgentRequest`
/// in `runtara-server/src/api/dto/agent_testing.rs` so wiring is a near-pass-
/// through.
#[derive(Debug, Clone)]
pub struct TestCapabilityRequest {
    pub tenant_id: String,
    pub agent_id: String,
    pub capability_id: String,
    pub input: serde_json::Value,
    pub connection: Option<ResolvedConnection>,
}

/// A connection record resolved by the host before invoke. Mirrors today's
/// `ConnectionsFacade::get_with_parameters` output.
#[derive(Debug, Clone)]
pub struct ResolvedConnection {
    pub connection_id: String,
    pub integration_id: String,
    pub connection_subtype: Option<String>,
    pub parameters: serde_json::Value,
    pub rate_limit_config: Option<serde_json::Value>,
}

/// Result shape returned to the server. Mirrors today's `TestResult` in
/// `runtara-server/src/api/dto/agent_testing.rs`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestResult {
    pub success: bool,
    pub output: Option<serde_json::Value>,
    pub error: Option<TestError>,
    pub execution_time_ms: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestError {
    pub code: String,
    pub message: String,
    pub category: String,
    pub severity: String,
    pub retryable: bool,
}

/// Routing context shared across calls — proxy URL, agent-service URL, etc.
/// Per-tenant fields go into `TestCapabilityRequest`.
#[derive(Debug, Clone)]
pub struct DispatcherEnv {
    pub proxy_url: String,
    pub agent_service_url: String,
    pub object_model_url: String,
    pub core_http_url: String,
}

pub struct ComponentDispatcherService {
    engine: Arc<Engine>,
    agents: HashMap<String, Arc<LoadedAgent>>,
    /// Per-agent capability metadata, cached at load time. Populated by an
    /// initial `list-capabilities` call against each component.
    metadata: HashMap<String, Vec<CapabilityInfo>>,
    env: DispatcherEnv,
}

impl ComponentDispatcherService {
    /// Build the service from a directory of `runtara_agent_*.wasm` files.
    /// The filename stem after the `runtara_agent_` prefix becomes the agent
    /// id (e.g. `runtara_agent_crypto.wasm` → agent id `crypto`).
    pub async fn from_dir(component_dir: &Path, env: DispatcherEnv) -> Result<Self> {
        let engine = build_engine(&EngineConfig::default())?;
        let linker = build_linker(&engine)?;

        let mut agents = HashMap::new();
        let mut metadata = HashMap::new();

        let entries = std::fs::read_dir(component_dir)
            .with_context(|| format!("read component directory {}", component_dir.display()))?;
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("wasm") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let Some(agent_id) = stem.strip_prefix("runtara_agent_") else {
                continue;
            };
            let agent_id = agent_id.to_string();

            let loaded = load_agent(&engine, &linker, &path, &agent_id)?;

            // Capability enumeration: one throwaway store at load time.
            let caps = enumerate_capabilities(&engine, &loaded).await?;
            metadata.insert(agent_id.clone(), caps);
            agents.insert(agent_id, loaded);
        }

        // Linker is consumed by `linker.instantiate_pre`; after every agent
        // is pre-instantiated we drop it — InstancePre carries everything we
        // need for repeated per-call instantiation.
        drop(linker);

        Ok(Self {
            engine,
            agents,
            metadata,
            env,
        })
    }

    /// Whether the dispatcher knows about an agent. Used by the server-side
    /// routing decision: components-mode for known agents, legacy fallback
    /// for the rest.
    pub fn has_agent(&self, agent_id: &str) -> bool {
        self.agents.contains_key(agent_id)
    }

    /// All loaded agent ids.
    pub fn agent_ids(&self) -> impl Iterator<Item = &str> {
        self.agents.keys().map(String::as_str)
    }

    /// Capability metadata for one agent, populated at load time.
    pub fn capabilities_of(&self, agent_id: &str) -> Option<&[CapabilityInfo]> {
        self.metadata.get(agent_id).map(|v| v.as_slice())
    }

    /// Execute one capability and return a `TestResult` shaped for the
    /// server's existing `TestResult` DTO.
    pub async fn test_capability(&self, req: TestCapabilityRequest) -> Result<TestResult> {
        let agent = self
            .agents
            .get(&req.agent_id)
            .with_context(|| format!("unknown agent `{}`", req.agent_id))?;

        let conn = req.connection.as_ref().map(|c| ConnectionInfo {
            connection_id: c.connection_id.clone(),
            integration_id: c.integration_id.clone(),
            connection_subtype: c.connection_subtype.clone(),
            parameters: serde_json::to_string(&c.parameters).unwrap_or_else(|_| "{}".into()),
            rate_limit_config: c
                .rate_limit_config
                .as_ref()
                .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "{}".into())),
        });
        let input_json = serde_json::to_string(&req.input)?;

        let ctx = Arc::new(CallContext::for_test(
            &req.tenant_id,
            &self.env.proxy_url,
            &self.env.agent_service_url,
            &self.env.object_model_url,
            &self.env.core_http_url,
        ));
        let state = HostState::new(ctx);
        let (mut store, agent_handle) = instantiate(&self.engine, &agent.pre, state).await?;

        let started = std::time::Instant::now();
        let result = agent_handle
            .runtara_agent_capabilities()
            .call_invoke(&mut store, &req.capability_id, &input_json, conn.as_ref())
            .await?;
        let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;

        Ok(match result {
            Ok(out_json) => TestResult {
                success: true,
                output: serde_json::from_str(&out_json).ok(),
                error: None,
                execution_time_ms: elapsed_ms,
            },
            Err(e) => TestResult {
                success: false,
                output: None,
                error: Some(TestError {
                    code: e.code,
                    message: e.message,
                    category: e.category,
                    severity: e.severity,
                    retryable: e.retryable,
                }),
                execution_time_ms: elapsed_ms,
            },
        })
    }
}

async fn enumerate_capabilities(
    engine: &Engine,
    loaded: &LoadedAgent,
) -> Result<Vec<CapabilityInfo>> {
    let state = HostState::new(Arc::new(CallContext::placeholder_for_metadata()));
    let (mut store, agent) = instantiate(engine, &loaded.pre, state).await?;
    let caps = agent
        .runtara_agent_capabilities()
        .call_list_capabilities(&mut store)
        .await?;
    Ok(caps)
}
