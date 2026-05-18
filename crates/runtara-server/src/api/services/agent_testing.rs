//! Agent Testing Service
//!
//! Business logic for testing agents. In Phase 1 of the WASM Components
//! migration, this service supports two execution backends side-by-side:
//!
//! 1. **Components** — embedded wasmtime + `runtara_agent_*.wasm` components
//!    loaded from `agent_components_dir`. See `runtara-component-host`.
//! 2. **Legacy** — the universal dispatcher image (`__agent_dispatcher__:N`)
//!    executed via `runtime_client.execute_sync`. Removed in Phase 4.

use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tracing::{info, warn};

use opentelemetry::KeyValue;
use runtara_component_host::{
    ComponentDispatcherService, ResolvedConnection, TestCapabilityRequest,
};

use crate::api::dto::agent_testing::TestEngine;
use crate::observability::metrics;

use super::dispatcher::DispatcherService;

#[derive(Debug)]
pub enum ServiceError {
    NotEnabled,
    RateLimitExceeded(Duration),
    AgentNotFound(String),
    ExecutionError(String),
    ConnectionNotFound(String),
    DatabaseError(String),
}

impl std::fmt::Display for ServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServiceError::NotEnabled => write!(f, "Agent testing is not enabled"),
            ServiceError::RateLimitExceeded(wait_time) => {
                write!(
                    f,
                    "Rate limit exceeded. Wait {:.2}s",
                    wait_time.as_secs_f64()
                )
            }
            ServiceError::AgentNotFound(msg) => write!(f, "Agent not found: {}", msg),
            ServiceError::ExecutionError(msg) => write!(f, "Execution error: {}", msg),
            ServiceError::ConnectionNotFound(msg) => write!(f, "Connection not found: {}", msg),
            ServiceError::DatabaseError(msg) => write!(f, "Database error: {}", msg),
        }
    }
}

/// Rate limiter that enforces 1 request per second per agent
#[derive(Clone)]
struct RateLimiter {
    last_calls: Arc<Mutex<HashMap<String, Instant>>>,
}

impl RateLimiter {
    fn new() -> Self {
        Self {
            last_calls: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Check if the rate limit allows this request
    /// Returns Ok(()) if allowed, Err(Duration) with wait time if not
    fn check_rate_limit(&self, agent: &str, capability: &str) -> Result<(), Duration> {
        let key = format!("{}:{}", agent, capability);
        let mut calls = self.last_calls.lock().unwrap();

        if let Some(last_call) = calls.get(&key) {
            let elapsed = last_call.elapsed();
            if elapsed < Duration::from_secs(1) {
                let wait_time = Duration::from_secs(1) - elapsed;
                return Err(wait_time);
            }
        }

        calls.insert(key, Instant::now());
        Ok(())
    }
}

/// Response from agent test execution
#[derive(Debug)]
pub struct TestResult {
    pub success: bool,
    pub output: Option<Value>,
    pub error: Option<String>,
    pub execution_time_ms: f64,
    pub max_memory_mb: Option<f64>,
    /// Which engine actually executed this call. Surfaces in the response so
    /// CI A/B harnesses can confirm routing.
    pub engine: ActiveEngine,
}

/// The engine actually used for a given call — what the service decided
/// after honoring the requested `TestEngine` and the available backends.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, utoipa::ToSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum ActiveEngine {
    Components,
    Legacy,
}

/// Agent testing service. Holds both backends so callers can compare them.
#[derive(Clone)]
pub struct AgentTestingService {
    enabled: bool,
    rate_limiter: RateLimiter,
    dispatcher_service: Option<Arc<DispatcherService>>,
    component_dispatcher: Option<Arc<ComponentDispatcherService>>,
    connections: Option<Arc<runtara_connections::ConnectionsFacade>>,
}

impl AgentTestingService {
    /// Create a new agent testing service.
    pub fn new(enabled: bool, dispatcher_service: Option<Arc<DispatcherService>>) -> Self {
        Self {
            enabled,
            rate_limiter: RateLimiter::new(),
            dispatcher_service,
            component_dispatcher: None,
            connections: None,
        }
    }

    pub fn with_connections(mut self, facade: Arc<runtara_connections::ConnectionsFacade>) -> Self {
        self.connections = Some(facade);
        self
    }

    /// Plug in the embedded component dispatcher. When set, agents with a
    /// loaded `.wasm` component go through it unless `engine=legacy` is
    /// forced.
    pub fn with_component_dispatcher(
        mut self,
        dispatcher: Arc<ComponentDispatcherService>,
    ) -> Self {
        self.component_dispatcher = Some(dispatcher);
        self
    }

    /// Check if agent testing is enabled
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Resolve which engine to use given the request preference and the
    /// agent's availability in the component dispatcher.
    fn pick_engine(
        &self,
        requested: TestEngine,
        agent_name: &str,
    ) -> Result<ActiveEngine, ServiceError> {
        let has_component = match self.component_dispatcher.as_deref() {
            Some(d) => d.has_agent(agent_name),
            None => false,
        };
        match requested {
            TestEngine::Components => {
                if has_component {
                    Ok(ActiveEngine::Components)
                } else {
                    Err(ServiceError::AgentNotFound(format!(
                        "agent `{}` has no WASM component loaded",
                        agent_name
                    )))
                }
            }
            TestEngine::Legacy => Ok(ActiveEngine::Legacy),
            TestEngine::Auto => {
                if has_component {
                    Ok(ActiveEngine::Components)
                } else {
                    Ok(ActiveEngine::Legacy)
                }
            }
        }
    }

    /// Execute an agent with the given input.
    ///
    /// # Arguments
    /// * `tenant_id` - The tenant identifier
    /// * `agent_name` - The agent module name (e.g., "utils", "http")
    /// * `capability_id` - The capability ID (e.g., "random-double", "hash")
    /// * `input` - The agent-specific input as JSON
    /// * `connection_id` - Optional connection ID for agents requiring credentials
    /// * `engine` - Preferred execution backend (Auto, Components, or Legacy)
    pub async fn test_agent(
        &self,
        tenant_id: &str,
        agent_name: &str,
        capability_id: &str,
        input: Value,
        connection_id: Option<String>,
        engine: TestEngine,
    ) -> Result<TestResult, ServiceError> {
        if !self.enabled {
            return Err(ServiceError::NotEnabled);
        }

        if let Err(wait_time) = self
            .rate_limiter
            .check_rate_limit(agent_name, capability_id)
        {
            return Err(ServiceError::RateLimitExceeded(wait_time));
        }

        let active = self.pick_engine(engine, agent_name)?;
        info!(
            tenant_id = %tenant_id,
            agent = %agent_name,
            capability = %capability_id,
            engine = ?active,
            "Executing agent test"
        );

        let result = match active {
            ActiveEngine::Components => {
                self.run_via_components(tenant_id, agent_name, capability_id, input, connection_id)
                    .await
            }
            ActiveEngine::Legacy => {
                self.run_via_legacy_dispatcher(
                    tenant_id,
                    agent_name,
                    capability_id,
                    input,
                    connection_id,
                )
                .await
            }
        };

        // Per-engine telemetry: count + duration histogram, labeled by
        // engine/agent/capability so dashboards can A/B during the migration.
        if let Some(m) = metrics() {
            let engine_label = match active {
                ActiveEngine::Components => "components",
                ActiveEngine::Legacy => "legacy",
            };
            let attrs = [
                KeyValue::new("engine", engine_label),
                KeyValue::new("agent", agent_name.to_string()),
                KeyValue::new("capability", capability_id.to_string()),
                KeyValue::new("tenant_id", tenant_id.to_string()),
            ];
            m.agent_test_total.add(1, &attrs);
            match &result {
                Ok(r) => {
                    m.agent_test_duration
                        .record(r.execution_time_ms / 1000.0, &attrs);
                    if !r.success {
                        m.agent_test_failed.add(1, &attrs);
                    }
                }
                Err(_) => {
                    m.agent_test_failed.add(1, &attrs);
                }
            }
        }

        result
    }

    async fn run_via_components(
        &self,
        tenant_id: &str,
        agent_name: &str,
        capability_id: &str,
        input: Value,
        connection_id: Option<String>,
    ) -> Result<TestResult, ServiceError> {
        let dispatcher = self
            .component_dispatcher
            .as_ref()
            .expect("pick_engine guards against missing dispatcher");

        let connection = match connection_id.as_deref() {
            Some(id) => Some(self.resolve_connection(tenant_id, id).await?),
            None => None,
        };

        let req = TestCapabilityRequest {
            tenant_id: tenant_id.to_string(),
            agent_id: agent_name.to_string(),
            capability_id: capability_id.to_string(),
            input,
            connection,
        };
        let result = dispatcher
            .test_capability(req)
            .await
            .map_err(|e| ServiceError::ExecutionError(format!("Components: {}", e)))?;

        Ok(TestResult {
            success: result.success,
            output: result.output,
            error: result.error.map(|e| format!("{}: {}", e.code, e.message)),
            execution_time_ms: result.execution_time_ms,
            max_memory_mb: None,
            engine: ActiveEngine::Components,
        })
    }

    async fn run_via_legacy_dispatcher(
        &self,
        tenant_id: &str,
        agent_name: &str,
        capability_id: &str,
        input: Value,
        connection_id: Option<String>,
    ) -> Result<TestResult, ServiceError> {
        let dispatcher = self.dispatcher_service.as_ref().ok_or_else(|| {
            ServiceError::ExecutionError(
                "Dispatcher service not configured. Runtime client may not be available."
                    .to_string(),
            )
        })?;

        let image_id = dispatcher
            .get_dispatcher_image(tenant_id)
            .await
            .map_err(|e| ServiceError::ExecutionError(format!("Dispatcher not ready: {}", e)))?;

        let connection_data = match connection_id.as_deref() {
            Some(id) => Some(self.load_connection_legacy_blob(tenant_id, id).await?),
            None => None,
        };

        let dispatcher_input = serde_json::json!({
            "agent_id": agent_name,
            "capability_id": capability_id,
            "agent_input": input,
            "connection": connection_data,
        });

        let start = Instant::now();
        let result = dispatcher
            .runtime_client()
            .execute_sync(
                &image_id,
                tenant_id,
                "agent-dispatcher",
                None,
                Some(dispatcher_input),
                Some(30),
                false,
            )
            .await
            .map_err(|e| ServiceError::ExecutionError(format!("Execution failed: {}", e)))?;
        let execution_time_ms = start.elapsed().as_secs_f64() * 1000.0;

        if !result.success {
            let error_msg = result
                .error
                .unwrap_or_else(|| "Execution failed".to_string());
            warn!(
                tenant_id = %tenant_id,
                agent = %agent_name,
                error = %error_msg,
                "Agent test execution failed (legacy)"
            );
            return Err(ServiceError::ExecutionError(error_msg));
        }

        let output = result.output.ok_or_else(|| {
            ServiceError::ExecutionError("Dispatcher returned no output".to_string())
        })?;
        let success = output
            .get("success")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let agent_output = output.get("output").cloned();
        let agent_error = output
            .get("error")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        if success {
            Ok(TestResult {
                success: true,
                output: agent_output,
                error: None,
                execution_time_ms,
                max_memory_mb: None,
                engine: ActiveEngine::Legacy,
            })
        } else {
            let err_msg = agent_error.unwrap_or_else(|| "Unknown agent error".to_string());
            if err_msg.contains("Unknown agent") || err_msg.contains("Unknown capability") {
                Err(ServiceError::AgentNotFound(err_msg))
            } else if err_msg.contains("Connection") && err_msg.contains("not found") {
                Err(ServiceError::ConnectionNotFound(err_msg))
            } else {
                Ok(TestResult {
                    success: false,
                    output: None,
                    error: Some(err_msg),
                    execution_time_ms,
                    max_memory_mb: None,
                    engine: ActiveEngine::Legacy,
                })
            }
        }
    }

    /// Resolve a connection record into the strongly-typed shape the
    /// component dispatcher expects.
    async fn resolve_connection(
        &self,
        tenant_id: &str,
        connection_id: &str,
    ) -> Result<ResolvedConnection, ServiceError> {
        let facade = self.connections.as_ref().ok_or_else(|| {
            ServiceError::DatabaseError("ConnectionsFacade not configured".to_string())
        })?;
        let conn = facade
            .get_with_parameters(connection_id, tenant_id)
            .await
            .map_err(|e| ServiceError::DatabaseError(format!("Failed to query connection: {}", e)))?
            .ok_or_else(|| {
                ServiceError::ConnectionNotFound(format!(
                    "Connection '{}' not found for tenant '{}'",
                    connection_id, tenant_id
                ))
            })?;
        let integration_id = conn.integration_id.ok_or_else(|| {
            ServiceError::ExecutionError(format!(
                "Connection '{}' has no integration_id configured",
                connection_id
            ))
        })?;
        Ok(ResolvedConnection {
            connection_id: connection_id.to_string(),
            integration_id,
            connection_subtype: conn.connection_subtype,
            parameters: conn
                .connection_parameters
                .unwrap_or_else(|| serde_json::json!({})),
            rate_limit_config: conn.rate_limit_config,
        })
    }

    /// Legacy path connection lookup — returns the same JSON blob shape the
    /// dispatcher binary expects (`{integration_id, connection_subtype,
    /// parameters, rate_limit_config}`).
    async fn load_connection_legacy_blob(
        &self,
        tenant_id: &str,
        connection_id: &str,
    ) -> Result<Value, ServiceError> {
        let resolved = self.resolve_connection(tenant_id, connection_id).await?;
        Ok(serde_json::json!({
            "integration_id": resolved.integration_id,
            "connection_subtype": resolved.connection_subtype,
            "parameters": resolved.parameters,
            "rate_limit_config": resolved.rate_limit_config,
        }))
    }
}
