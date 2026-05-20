//! `ComponentDispatcherService` — the host-facing API that
//! `AgentTestingService` calls into instead of dispatcher-image roundtrips.
//!
//! Loads `runtara_agent_*.wasm` + `runtara_agent_*.meta.json` pairs from a
//! directory at construction time, pre-instantiates each `.wasm`, and serves
//! the parsed `AgentInfo` directly to the server. The `.wasm` exports only
//! `invoke`; all metadata travels through the sidecar JSON.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use runtara_dsl::agent_meta::AgentInfo;
use serde::{Deserialize, Serialize};
use wasmtime::Engine;

use crate::bindings::exports::runtara::agent::capabilities::{ConnectionInfo, ErrorInfo};
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
    /// Snapshot of every loaded agent's metadata. Shared (`Arc`) so the
    /// server-side `AgentsService` + workflow validation paths can hold the
    /// same data without copying.
    catalog: Arc<runtara_dsl::agent_meta::AgentCatalog>,
    env: DispatcherEnv,
}

impl ComponentDispatcherService {
    /// Build the service from a directory of `runtara_agent_*.wasm` files,
    /// each accompanied by a sibling `runtara_agent_*.meta.json`. The filename
    /// stem after the `runtara_agent_` prefix becomes the agent id (e.g.
    /// `runtara_agent_crypto.wasm` → agent id `crypto`).
    ///
    /// A missing `.meta.json` is a hard error — the `.wasm` is unusable to the
    /// server without metadata. Mismatched ids (filename stem vs.
    /// `meta.id`) are also rejected so registration can't silently misroute.
    pub async fn from_dir(component_dir: &Path, env: DispatcherEnv) -> Result<Self> {
        let engine = build_engine(&EngineConfig::default())?;
        let linker = build_linker(&engine)?;

        let mut agents = HashMap::new();
        let mut agent_info: HashMap<String, AgentInfo> = HashMap::new();

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
            let Some(stem_id) = stem.strip_prefix("runtara_agent_") else {
                continue;
            };
            // cargo-component drops the .wasm filename in snake_case (it
            // converts hyphens in the crate name to underscores). The
            // canonical agent id everywhere else — `meta.json`,
            // `AgentsService`, workflow DSL refs — is kebab. Convert here so
            // both halves of the bundle agree on the id format.
            let agent_id = stem_id.replace('_', "-");

            let meta_path = path.with_extension("meta.json");
            let meta_bytes = std::fs::read(&meta_path).with_context(|| {
                format!(
                    "agent `{agent_id}`: missing sidecar metadata at {}",
                    meta_path.display()
                )
            })?;
            let mut info: AgentInfo = serde_json::from_slice(&meta_bytes).with_context(|| {
                format!(
                    "agent `{agent_id}`: failed to parse sidecar metadata {}",
                    meta_path.display()
                )
            })?;
            // Normalize both sides to kebab for the equality check —
            // cargo-component drops snake_case filenames, but the canonical
            // id everywhere else is kebab, so an agent crate can sensibly
            // write either form in its `agent_info().id` literal.
            if info.id.replace('_', "-") != agent_id {
                anyhow::bail!(
                    "agent id mismatch: filename stem is `{agent_id}` but meta.id is `{}`",
                    info.id
                );
            }
            // Force the catalog to key on the same kebab form `agents` uses
            // — otherwise an agent whose `agent_info().id` literal is
            // snake_case (e.g. "azure_blob_storage") loads into `agents` as
            // "azure-blob-storage" but registers in the catalog as
            // "azure_blob_storage", and `agent_info_of("azure-blob-storage")`
            // returns None while `agent_ids()` yields it.
            info.id = agent_id.clone();

            let loaded = load_agent(&engine, &linker, &path, &agent_id)?;

            agent_info.insert(agent_id.clone(), info);
            agents.insert(agent_id, loaded);
        }

        // Linker is consumed by `linker.instantiate_pre`; after every agent
        // is pre-instantiated we drop it — InstancePre carries everything we
        // need for repeated per-call instantiation.
        drop(linker);

        // Build the public catalog from the parsed `AgentInfo`s. Sorted by
        // id so API output + tests are deterministic.
        let mut by_id: Vec<(String, AgentInfo)> = agent_info.into_iter().collect();
        by_id.sort_by(|a, b| a.0.cmp(&b.0));
        let catalog = Arc::new(runtara_dsl::agent_meta::AgentCatalog::from_agents(
            by_id.into_iter().map(|(_, v)| v).collect(),
        ));

        Ok(Self {
            engine,
            agents,
            catalog,
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

    /// Full metadata for one agent (parsed from its sidecar `meta.json`).
    pub fn agent_info_of(&self, agent_id: &str) -> Option<&AgentInfo> {
        self.catalog.agent(agent_id)
    }

    /// The shared agent catalog. Server-side validators + the
    /// `AgentsService` consume this instead of `runtara_agents::registry`
    /// so the runtime, not compile-time, is the source of truth.
    pub fn catalog(&self) -> Arc<runtara_dsl::agent_meta::AgentCatalog> {
        Arc::clone(&self.catalog)
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
        let input_bytes = serde_json::to_vec(&req.input)?;

        let ctx = Arc::new(CallContext::for_test(
            &req.tenant_id,
            &self.env.proxy_url,
            &self.env.agent_service_url,
            &self.env.object_model_url,
            &self.env.core_http_url,
        ));
        let state = HostState::new(ctx);
        let (mut store, instance) = instantiate(&self.engine, &agent.pre, state).await?;

        // Dynamic dispatch: look up the agent's capabilities interface by the
        // name we cached at load time (`runtara:agent-<id>/capabilities@…` for
        // per-agent WIT, `runtara:agent/capabilities@…` for the legacy
        // shared-WIT layout), then resolve `invoke` inside it and call with
        // the canonical signature.
        let iface_idx = instance
            .get_export_index(&mut store, None, &agent.capabilities_iface)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "agent `{}` instance is missing the `{}` interface export",
                    req.agent_id,
                    agent.capabilities_iface
                )
            })?;
        let invoke_idx = instance
            .get_export_index(&mut store, Some(&iface_idx), "invoke")
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "agent `{}` `{}` interface has no `invoke` export",
                    req.agent_id,
                    agent.capabilities_iface
                )
            })?;
        type InvokeFunc = wasmtime::component::TypedFunc<
            (String, Vec<u8>, Option<ConnectionInfo>),
            (Result<Vec<u8>, ErrorInfo>,),
        >;
        let invoke: InvokeFunc = instance.get_typed_func(&mut store, invoke_idx)?;

        let started = std::time::Instant::now();
        let (result,) = invoke
            .call_async(&mut store, (req.capability_id.clone(), input_bytes, conn))
            .await?;
        let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;

        Ok(match result {
            Ok(out_bytes) => TestResult {
                success: true,
                output: serde_json::from_slice(&out_bytes).ok(),
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
