//! Agent Execution Service
//!
//! Executes agent capabilities directly in the host process on behalf of
//! scenario instances. This is the host-mediated I/O path: scenarios call
//! this service via HTTP instead of running agents in-process.
//!
//! Unlike agent_testing (which uses the OCI dispatcher container), this service
//! calls `execute_capability` directly — the agent inventory is already linked
//! into the runtara-server binary.

use serde_json::Value;
use sqlx::PgPool;
use std::time::Instant;
use tracing::{info, warn};

/// Errors from agent execution
#[derive(Debug)]
pub enum AgentExecutionError {
    /// Agent or capability not found in registry
    AgentNotFound(String),
    /// Connection not found in database
    ConnectionNotFound(String),
    /// Agent returned an error during execution
    ExecutionFailed(String),
    /// Database query failed
    DatabaseError(String),
}

impl std::fmt::Display for AgentExecutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AgentNotFound(msg) => write!(f, "Agent not found: {}", msg),
            Self::ConnectionNotFound(msg) => write!(f, "Connection not found: {}", msg),
            Self::ExecutionFailed(msg) => write!(f, "Execution failed: {}", msg),
            Self::DatabaseError(msg) => write!(f, "Database error: {}", msg),
        }
    }
}

/// Result from executing an agent capability
#[derive(Debug)]
pub struct ExecutionResult {
    pub success: bool,
    pub output: Option<Value>,
    pub error: Option<String>,
    pub execution_time_ms: f64,
}

/// Agent execution service — runs agent capabilities directly in the host process.
#[derive(Clone)]
pub struct AgentExecutionService {
    pool: PgPool,
}

impl AgentExecutionService {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Execute an agent capability with the given input.
    ///
    /// Connection is resolved from the database if `connection_id` is provided,
    /// and injected as the `_connection` field in the agent input (matching the
    /// convention used by the OCI dispatcher).
    pub async fn execute(
        &self,
        tenant_id: &str,
        agent_id: &str,
        capability_id: &str,
        mut inputs: Value,
        connection_id: Option<&str>,
    ) -> Result<ExecutionResult, AgentExecutionError> {
        // Resolve connection if provided
        if let Some(conn_id) = connection_id {
            let connection_data = self.load_connection(tenant_id, conn_id).await?;

            // Inject as _connection field (matches dispatcher convention)
            if let Some(obj) = inputs.as_object_mut() {
                obj.insert("_connection".to_string(), connection_data);
            }
        }

        info!(
            tenant_id = %tenant_id,
            agent = %agent_id,
            capability = %capability_id,
            "Executing agent capability on host"
        );

        let start = Instant::now();

        // Execute directly via the inventory-based registry.
        // All agents are linked into runtara-server at compile time.
        let result = runtara_dsl::agent_meta::execute_capability(agent_id, capability_id, inputs);

        let execution_time_ms = start.elapsed().as_secs_f64() * 1000.0;

        match result {
            Ok(output) => {
                info!(
                    agent = %agent_id,
                    capability = %capability_id,
                    time_ms = %execution_time_ms,
                    "Agent capability executed successfully"
                );
                Ok(ExecutionResult {
                    success: true,
                    output: Some(output),
                    error: None,
                    execution_time_ms,
                })
            }
            Err(err_str) => {
                // Check if this is an "unknown agent" error from the registry
                if err_str.contains("Unknown capability") || err_str.contains("Unknown agent") {
                    return Err(AgentExecutionError::AgentNotFound(err_str));
                }

                warn!(
                    agent = %agent_id,
                    capability = %capability_id,
                    error = %err_str,
                    time_ms = %execution_time_ms,
                    "Agent capability execution failed"
                );

                // Agent errors are returned as a successful HTTP response with success=false,
                // not as HTTP errors. This matches the convention used by the dispatcher.
                // The caller (scenario instance) can then decide how to handle the error
                // (retry via #[durable], propagate to user, etc.).
                Ok(ExecutionResult {
                    success: false,
                    output: None,
                    error: Some(err_str),
                    execution_time_ms,
                })
            }
        }
    }

    /// Load connection credentials from the database.
    ///
    /// Returns a JSON object with `integration_id`, `connection_subtype`,
    /// `parameters`, and `rate_limit_config` — matching the `RawConnection`
    /// structure expected by agents.
    async fn load_connection(
        &self,
        tenant_id: &str,
        connection_id: &str,
    ) -> Result<Value, AgentExecutionError> {
        let row: Option<ConnectionRow> = sqlx::query_as(
            r#"
            SELECT
                connection_subtype,
                integration_id,
                connection_parameters as parameters,
                rate_limit_config
            FROM connection_data_entity
            WHERE tenant_id = $1 AND id = $2
            "#,
        )
        .bind(tenant_id)
        .bind(connection_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            AgentExecutionError::DatabaseError(format!("Failed to query connection: {}", e))
        })?;

        let row = row.ok_or_else(|| {
            AgentExecutionError::ConnectionNotFound(format!(
                "Connection '{}' not found for tenant '{}'",
                connection_id, tenant_id
            ))
        })?;

        let integration_id = row.integration_id.ok_or_else(|| {
            AgentExecutionError::ExecutionFailed(format!(
                "Connection '{}' has no integration_id configured",
                connection_id
            ))
        })?;

        Ok(serde_json::json!({
            "integration_id": integration_id,
            "connection_subtype": row.connection_subtype,
            "parameters": row.parameters,
            "rate_limit_config": row.rate_limit_config
        }))
    }
}

/// Row structure for connection query
#[derive(sqlx::FromRow)]
struct ConnectionRow {
    connection_subtype: Option<String>,
    integration_id: Option<String>,
    parameters: Option<Value>,
    rate_limit_config: Option<Value>,
}
