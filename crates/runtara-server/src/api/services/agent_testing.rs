//! Agent Testing Service
//!
//! Business logic for testing agents using sandboxed container execution.
//! Agents are executed via the universal dispatcher in runtara-environment.

use serde_json::Value;
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tracing::{info, warn};

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
}

/// Agent testing service
///
/// Executes individual agents in isolation via the universal dispatcher.
#[derive(Clone)]
pub struct AgentTestingService {
    enabled: bool,
    rate_limiter: RateLimiter,
    dispatcher_service: Option<Arc<DispatcherService>>,
    pool: Option<PgPool>,
    connections: Option<Arc<runtara_connections::ConnectionsFacade>>,
}

impl AgentTestingService {
    /// Create a new agent testing service
    ///
    /// # Arguments
    /// * `enabled` - Whether agent testing is enabled
    /// * `dispatcher_service` - Optional dispatcher service for executing agents
    /// * `pool` - Optional database pool for loading connections
    pub fn new(
        enabled: bool,
        dispatcher_service: Option<Arc<DispatcherService>>,
        pool: Option<PgPool>,
    ) -> Self {
        Self {
            enabled,
            rate_limiter: RateLimiter::new(),
            dispatcher_service,
            pool,
            connections: None,
        }
    }

    pub fn with_connections(
        mut self,
        facade: Arc<runtara_connections::ConnectionsFacade>,
    ) -> Self {
        self.connections = Some(facade);
        self
    }

    /// Check if agent testing is enabled
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Execute an agent with the given input in a sandboxed container.
    ///
    /// # Arguments
    /// * `tenant_id` - The tenant identifier
    /// * `agent_name` - The agent module name (e.g., "utils", "http")
    /// * `capability_id` - The capability ID (e.g., "random-double", "http-request")
    /// * `input` - The agent-specific input as JSON
    /// * `connection_id` - Optional connection ID for agents requiring credentials
    ///
    /// # Returns
    /// Result with test execution results or an error
    pub async fn test_agent(
        &self,
        tenant_id: &str,
        agent_name: &str,
        capability_id: &str,
        input: Value,
        connection_id: Option<String>,
    ) -> Result<TestResult, ServiceError> {
        // Check if agent testing is enabled
        if !self.enabled {
            return Err(ServiceError::NotEnabled);
        }

        // Check rate limit
        if let Err(wait_time) = self
            .rate_limiter
            .check_rate_limit(agent_name, capability_id)
        {
            return Err(ServiceError::RateLimitExceeded(wait_time));
        }

        // Get dispatcher service
        let dispatcher = self.dispatcher_service.as_ref().ok_or_else(|| {
            ServiceError::ExecutionError(
                "Dispatcher service not configured. Runtime client may not be available."
                    .to_string(),
            )
        })?;

        // Get the pre-initialized dispatcher image
        let image_id = dispatcher
            .get_dispatcher_image(tenant_id)
            .await
            .map_err(|e| ServiceError::ExecutionError(format!("Dispatcher not ready: {}", e)))?;

        // Load connection data if connection_id is provided
        let connection_data = if let Some(ref conn_id) = connection_id {
            Some(self.load_connection(tenant_id, conn_id).await?)
        } else {
            None
        };

        // Build dispatcher input
        let dispatcher_input = serde_json::json!({
            "agent_id": agent_name,
            "capability_id": capability_id,
            "agent_input": input,
            "connection": connection_data,
        });

        info!(
            tenant_id = %tenant_id,
            agent = %agent_name,
            capability = %capability_id,
            "Executing agent test via dispatcher"
        );

        let start = Instant::now();

        // Execute via runtime client
        let result = dispatcher
            .runtime_client()
            .execute_sync(
                &image_id,
                tenant_id,
                "agent-dispatcher", // Scenario ID for tracing context
                None,               // Generate instance_id automatically
                Some(dispatcher_input),
                Some(30), // 30 second timeout for agent tests
                false,
            )
            .await
            .map_err(|e| ServiceError::ExecutionError(format!("Execution failed: {}", e)))?;

        let execution_time_ms = start.elapsed().as_secs_f64() * 1000.0;

        // Parse dispatcher output
        if result.success {
            if let Some(output) = result.output {
                // Dispatcher wraps output in { success: bool, output?: Value, error?: String }
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
                        max_memory_mb: None, // TODO: get from metrics when available
                    })
                } else {
                    // Agent returned an error
                    let err_msg = agent_error.unwrap_or_else(|| "Unknown agent error".to_string());

                    // Check for specific error types
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
                        })
                    }
                }
            } else {
                Err(ServiceError::ExecutionError(
                    "Dispatcher returned no output".to_string(),
                ))
            }
        } else {
            // Container execution failed
            let error_msg = result
                .error
                .unwrap_or_else(|| "Execution failed".to_string());
            warn!(
                tenant_id = %tenant_id,
                agent = %agent_name,
                error = %error_msg,
                "Agent test execution failed"
            );
            Err(ServiceError::ExecutionError(error_msg))
        }
    }

    /// Load connection data via the connections facade
    async fn load_connection(
        &self,
        tenant_id: &str,
        connection_id: &str,
    ) -> Result<Value, ServiceError> {
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

        // integration_id is required for agents to work
        let integration_id = conn.integration_id.ok_or_else(|| {
            ServiceError::ExecutionError(format!(
                "Connection '{}' has no integration_id configured",
                connection_id
            ))
        })?;

        // Build connection data for dispatcher (matches RawConnection structure)
        // This will be injected as _connection field in agent input
        let connection_data = serde_json::json!({
            "integration_id": integration_id,
            "connection_subtype": conn.connection_subtype,
            "parameters": conn.connection_parameters,
            "rate_limit_config": conn.rate_limit_config
        });

        Ok(connection_data)
    }
}
