//! Execution Engine
//!
//! Core execution logic extracted from native_worker for reuse by trigger workers.
//! Handles scenario compilation and execution via the Runtara Management SDK.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use dashmap::DashMap;
use serde_json::Value;
use sqlx::PgPool;
use tokio::sync::Mutex;
use tracing::{info, instrument};
use uuid::Uuid;

use runtara_management_sdk::InstanceStatus;

use crate::api::dto::trigger_event::TriggerEvent;
use crate::metrics::MetricsService;
use crate::runtime_client::RuntimeClient;
use crate::workers::CancellationHandle;

/// Result of scenario execution
#[derive(Debug)]
pub struct ExecutionResult {
    pub success: bool,
    pub output: Option<Value>,
    pub error: Option<String>,
    pub duration_seconds: f64,
    /// Peak memory usage in MB (from cgroup metrics)
    pub max_memory_mb: Option<f64>,
    /// CPU usage in milliseconds (from cgroup metrics)
    pub cpu_usage_ms: Option<f64>,
}

/// Errors that can occur during execution
#[derive(Debug)]
pub enum ExecutionError {
    ScenarioNotFound(String),
    CompilationFailed(String),
    BinaryNotFound(String),
    BundlePreparationFailed(String),
    RuntimeError(String),
    DatabaseError(String),
    NotConnected(String),
    /// Scenario is not compiled yet - should retry later
    NotCompiled {
        scenario_id: String,
        version: i32,
        /// Whether compilation was queued as a result of this check
        compilation_queued: bool,
    },
}

/// Inject `_scenario_id` into the inputs' variables to ensure cache key isolation.
///
/// This prevents cache key collisions when different scenarios have StartScenario steps
/// with the same step_id calling the same child scenario. The scenario_id becomes part
/// of the cache key prefix, ensuring each scenario's child executions are isolated.
fn inject_scenario_id(inputs: Value, scenario_id: &str) -> Value {
    let mut inputs = inputs;
    if let Some(obj) = inputs.as_object_mut() {
        // Get or create variables object
        let variables = obj
            .entry("variables")
            .or_insert_with(|| serde_json::json!({}));

        if let Some(vars_obj) = variables.as_object_mut() {
            vars_obj.insert(
                "_scenario_id".to_string(),
                serde_json::Value::String(scenario_id.to_string()),
            );
        }
    }
    inputs
}

impl std::fmt::Display for ExecutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecutionError::ScenarioNotFound(msg) => write!(f, "Scenario not found: {}", msg),
            ExecutionError::CompilationFailed(msg) => write!(f, "Compilation failed: {}", msg),
            ExecutionError::BinaryNotFound(msg) => write!(f, "Binary not found: {}", msg),
            ExecutionError::BundlePreparationFailed(msg) => {
                write!(f, "Bundle preparation failed: {}", msg)
            }
            ExecutionError::RuntimeError(msg) => write!(f, "Runtime error: {}", msg),
            ExecutionError::DatabaseError(msg) => write!(f, "Database error: {}", msg),
            ExecutionError::NotConnected(msg) => write!(f, "Not connected: {}", msg),
            ExecutionError::NotCompiled {
                scenario_id,
                version,
                compilation_queued,
            } => {
                write!(
                    f,
                    "Scenario '{}' version {} not compiled (compilation queued: {})",
                    scenario_id, version, compilation_queued
                )
            }
        }
    }
}

impl std::error::Error for ExecutionError {}

/// Execution engine for running scenarios
///
/// Handles execution via RuntimeClient. Scenarios MUST be pre-compiled via the compile API.
/// This engine does NOT compile - it only checks that compilation exists and executes.
pub struct ExecutionEngine {
    pool: PgPool,
    runtime_client: Option<Arc<RuntimeClient>>,
    #[allow(dead_code)] // Reserved for future use tracking in-memory executions
    running_executions: Option<Arc<DashMap<Uuid, CancellationHandle>>>,
    /// Track scenarios that are currently starting (to prevent single_instance race conditions)
    starting_scenarios: Arc<Mutex<HashSet<(String, String)>>>, // (tenant_id, scenario_id)
}

impl ExecutionEngine {
    /// Create a new execution engine
    pub fn new(
        pool: PgPool,
        runtime_client: Option<Arc<RuntimeClient>>,
        running_executions: Option<Arc<DashMap<Uuid, CancellationHandle>>>,
    ) -> Self {
        Self {
            pool,
            runtime_client,
            running_executions,
            starting_scenarios: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Check if the runtime client is available
    pub fn has_runtime(&self) -> bool {
        self.runtime_client.is_some()
    }

    /// Execute a scenario based on a trigger event
    ///
    /// This handles:
    /// 1. Checking if scenario is compiled
    /// 2. Compiling if needed
    /// 3. Running via Runtara Management SDK
    #[instrument(skip(self, event, _cancel_flag), fields(instance_id = %event.instance_id, scenario_id = %event.scenario_id))]
    pub async fn execute(
        &self,
        event: &TriggerEvent,
        _cancel_flag: Option<Arc<AtomicBool>>,
    ) -> Result<ExecutionResult, ExecutionError> {
        let runtime_client = self.runtime_client.as_ref().ok_or_else(|| {
            ExecutionError::NotConnected("Runtime client not configured".to_string())
        })?;

        let instance_id = Uuid::parse_str(&event.instance_id)
            .map_err(|e| ExecutionError::DatabaseError(format!("Invalid instance ID: {}", e)))?;

        // Resolve version if not specified
        let version = match event.version {
            Some(v) => v,
            None => {
                self.get_current_version(&event.tenant_id, &event.scenario_id)
                    .await?
            }
        };

        // Fetch execution timeout from executionGraph (if set)
        let execution_timeout_secs = sqlx::query!(
            r#"
            SELECT definition
            FROM scenario_definitions
            WHERE tenant_id = $1 AND scenario_id = $2 AND version = $3 AND deleted_at IS NULL
            "#,
            &event.tenant_id,
            &event.scenario_id,
            version
        )
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()
        .and_then(|r| {
            r.definition
                .get("executionTimeoutSeconds")
                .and_then(|v| {
                    // Handle both number and string formats
                    v.as_i64()
                        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                })
                .map(|secs| secs as u32)
        });

        // Check if scenario is compiled (binary exists on disk)
        let _binary_path = self
            .ensure_compiled(
                &event.tenant_id,
                &event.scenario_id,
                version,
                event.track_events,
            )
            .await?;

        // Note: Bundle checks removed - runtara-environment handles container execution

        // Execute via runtime client
        let exec_start = std::time::Instant::now();

        // Inputs are already in canonical format {"data": {...}, "variables": {...}}
        // from the API layer - inject _scenario_id for cache key isolation
        let scenario_input = inject_scenario_id(event.inputs.clone(), &event.scenario_id);

        // Get the registered image ID (UUID returned from runtara-environment)
        let image_id = self
            .get_registered_image_id(&event.tenant_id, &event.scenario_id, version)
            .await?;

        let result = runtime_client
            .execute_sync(
                &image_id,
                &event.tenant_id,
                &event.scenario_id,
                Some(instance_id.to_string()),
                Some(scenario_input),
                execution_timeout_secs,
                event.debug,
            )
            .await;

        let duration_seconds = exec_start.elapsed().as_secs_f64();

        // Record metrics and return result
        let metrics_service = MetricsService::new(self.pool.clone());

        match result {
            Ok(output) => {
                // Convert resource metrics from raw units to human-readable units
                let max_memory_mb = output
                    .memory_peak_bytes
                    .map(|bytes| bytes as f64 / 1_048_576.0);
                let cpu_usage_ms = output.cpu_usage_usec.map(|usec| usec as f64 / 1_000.0);

                // Record execution metrics
                let _ = metrics_service
                    .record_execution_completion(
                        &event.tenant_id,
                        &event.scenario_id,
                        version,
                        output.success,
                        duration_seconds,
                        max_memory_mb,
                    )
                    .await;

                Ok(ExecutionResult {
                    success: output.success,
                    output: output.output,
                    error: output.error,
                    duration_seconds,
                    max_memory_mb,
                    cpu_usage_ms,
                })
            }
            Err(e) => {
                // Record failed execution metrics
                let _ = metrics_service
                    .record_execution_completion(
                        &event.tenant_id,
                        &event.scenario_id,
                        version,
                        false,
                        duration_seconds,
                        None,
                    )
                    .await;

                Err(ExecutionError::RuntimeError(e.to_string()))
            }
        }
    }

    /// Execute a scenario in fire-and-forget mode for distributed execution.
    ///
    /// This method:
    /// 1. Ensures the scenario is compiled
    /// 2. Starts an instance via the Management SDK (non-blocking)
    /// 3. Returns immediately without waiting for completion
    ///
    /// The instance will run on the runtara-environment server.
    /// Use get_instance_status() to poll for completion.
    #[instrument(skip(self, event), fields(instance_id = %event.instance_id, scenario_id = %event.scenario_id))]
    pub async fn execute_detached(&self, event: &TriggerEvent) -> Result<String, ExecutionError> {
        // Mark scenario as starting (for single_instance race condition prevention)
        let scenario_key = (event.tenant_id.clone(), event.scenario_id.clone());
        {
            let mut starting = self.starting_scenarios.lock().await;
            starting.insert(scenario_key.clone());
        }

        // Execute and clean up starting_scenarios on completion (success or error)
        let result = self.execute_detached_inner(event).await;

        // Keep the scenario in starting_scenarios for a grace period to prevent race conditions
        // This ensures the database record has time to be created before we allow another instance
        let starting_scenarios = self.starting_scenarios.clone();
        let key = scenario_key.clone();
        tokio::spawn(async move {
            // Wait for DB record creation (execution typically takes 100-500ms to register)
            tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
            let mut starting = starting_scenarios.lock().await;
            starting.remove(&key);
        });

        result
    }

    /// Inner implementation of execute_detached
    async fn execute_detached_inner(&self, event: &TriggerEvent) -> Result<String, ExecutionError> {
        let runtime_client = self.runtime_client.as_ref().ok_or_else(|| {
            ExecutionError::NotConnected("Runtime client not configured".to_string())
        })?;

        // Resolve version if not specified
        let version = match event.version {
            Some(v) => v,
            None => {
                self.get_current_version(&event.tenant_id, &event.scenario_id)
                    .await?
            }
        };

        // Fetch execution timeout from executionGraph (if set)
        let execution_timeout_secs = sqlx::query!(
            r#"
            SELECT definition
            FROM scenario_definitions
            WHERE tenant_id = $1 AND scenario_id = $2 AND version = $3 AND deleted_at IS NULL
            "#,
            &event.tenant_id,
            &event.scenario_id,
            version
        )
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()
        .and_then(|r| {
            r.definition
                .get("executionTimeoutSeconds")
                .and_then(|v| {
                    v.as_i64()
                        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                })
                .map(|secs| secs as u32)
        })
        .unwrap_or(3600); // Default 1 hour timeout

        // Ensure scenario is compiled
        let _binary_path = self
            .ensure_compiled(
                &event.tenant_id,
                &event.scenario_id,
                version,
                event.track_events,
            )
            .await?;

        // Note: Bundle checks removed - runtara-environment handles container execution

        // Inputs are already in canonical format {"data": {...}, "variables": {...}}
        // from the API layer - inject _scenario_id for cache key isolation
        let scenario_input = inject_scenario_id(event.inputs.clone(), &event.scenario_id);

        // Get the registered image ID (UUID returned from runtara-environment)
        let image_id = self
            .get_registered_image_id(&event.tenant_id, &event.scenario_id, version)
            .await?;

        // Start instance (non-blocking)
        let started_id = match runtime_client
            .start_instance(
                &image_id,
                &event.tenant_id,
                &event.scenario_id,
                Some(event.instance_id.clone()),
                Some(scenario_input),
                Some(execution_timeout_secs),
                event.debug,
            )
            .await
        {
            Ok(id) => id,
            Err(e) => {
                let error_str = e.to_string();
                // Check if this is a stale image reference (image was purged from runtara-environment)
                if error_str.contains("image not found")
                    || error_str.contains("Image") && error_str.contains("not found")
                {
                    tracing::warn!(
                        tenant_id = %event.tenant_id,
                        scenario_id = %event.scenario_id,
                        version = version,
                        image_id = %image_id,
                        "Image not found in runtara-environment - deleting stale compilation record for recompilation"
                    );

                    // Delete the stale compilation record so it will be recompiled
                    let _ = sqlx::query!(
                        "DELETE FROM scenario_compilations WHERE tenant_id = $1 AND scenario_id = $2 AND version = $3",
                        &event.tenant_id,
                        &event.scenario_id,
                        version
                    )
                    .execute(&self.pool)
                    .await;

                    // Return NotCompiled to trigger recompilation queue
                    return Err(ExecutionError::NotCompiled {
                        scenario_id: event.scenario_id.clone(),
                        version,
                        compilation_queued: false, // Will be queued by caller on retry
                    });
                }
                return Err(ExecutionError::RuntimeError(error_str));
            }
        };

        info!(
            instance_id = %started_id,
            scenario_id = %event.scenario_id,
            version = version,
            "Started instance in detached mode"
        );

        Ok(started_id)
    }

    /// Check if a trigger has single_instance mode enabled
    ///
    /// Returns Some(true) if single_instance is enabled, Some(false) if disabled,
    /// or None if the trigger doesn't exist.
    pub async fn get_trigger_single_instance(
        &self,
        trigger_id: &str,
    ) -> Result<Option<bool>, ExecutionError> {
        let result = sqlx::query!(
            r#"
            SELECT single_instance
            FROM invocation_trigger
            WHERE id = $1
            "#,
            trigger_id
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| ExecutionError::DatabaseError(format!("Failed to get trigger: {}", e)))?;

        Ok(result.map(|r| r.single_instance))
    }

    /// Check if there's a running instance of a scenario
    ///
    /// Returns true if at least one running instance exists for the scenario.
    /// Used for single_instance trigger enforcement.
    ///
    /// Checks both:
    /// 1. In-memory starting_scenarios set (for instances being launched)
    /// 2. Runtara Management SDK for running instances
    pub async fn has_running_instance(
        &self,
        tenant_id: &str,
        scenario_id: &str,
    ) -> Result<bool, ExecutionError> {
        // First check in-memory set for scenarios currently being started
        // This prevents race conditions where multiple triggers are processed
        // before any database record is created
        {
            let starting = self.starting_scenarios.lock().await;
            if starting.contains(&(tenant_id.to_string(), scenario_id.to_string())) {
                return Ok(true);
            }
        }

        // Check runtara for running instances
        let runtime_client = match self.runtime_client.as_ref() {
            Some(client) => client,
            None => {
                // If no runtime client, we can't check runtara - assume no running instances
                return Ok(false);
            }
        };

        // Query runtara for running instances of this scenario
        // Image names follow the pattern "{scenario_id}:{version}", so we filter by prefix
        let result = runtime_client
            .list_instances_with_options(
                runtara_management_sdk::ListInstancesOptions::new()
                    .with_image_name_prefix(format!("{}:", scenario_id))
                    .with_status(InstanceStatus::Running)
                    .with_limit(1),
            )
            .await
            .map_err(|e| {
                ExecutionError::RuntimeError(format!("Failed to check running instances: {}", e))
            })?;

        Ok(!result.instances.is_empty())
    }

    /// Get the current version of a scenario
    async fn get_current_version(
        &self,
        tenant_id: &str,
        scenario_id: &str,
    ) -> Result<i32, ExecutionError> {
        let result = sqlx::query!(
            r#"
            SELECT COALESCE(current_version, latest_version) as "version"
            FROM scenarios
            WHERE tenant_id = $1 AND scenario_id = $2 AND deleted_at IS NULL
            "#,
            tenant_id,
            scenario_id
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| ExecutionError::DatabaseError(format!("Failed to get version: {}", e)))?;

        match result {
            Some(row) => row.version.ok_or_else(|| {
                ExecutionError::ScenarioNotFound(format!(
                    "Scenario '{}' has no versions",
                    scenario_id
                ))
            }),
            None => Err(ExecutionError::ScenarioNotFound(format!(
                "Scenario '{}' not found",
                scenario_id
            ))),
        }
    }

    /// Get the registered image ID for a compiled scenario
    ///
    /// Returns the UUID image_id that was assigned by runtara-environment
    /// during image registration. This is the ID that must be used for
    /// start_instance calls.
    async fn get_registered_image_id(
        &self,
        tenant_id: &str,
        scenario_id: &str,
        version: i32,
    ) -> Result<String, ExecutionError> {
        let result = sqlx::query!(
            r#"
            SELECT registered_image_id
            FROM scenario_compilations
            WHERE tenant_id = $1 AND scenario_id = $2 AND version = $3
                AND compilation_status = 'success'
            "#,
            tenant_id,
            scenario_id,
            version
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            ExecutionError::DatabaseError(format!("Failed to get registered image ID: {}", e))
        })?;

        result.and_then(|r| r.registered_image_id).ok_or_else(|| {
            ExecutionError::BinaryNotFound(format!(
                "Scenario '{}' version {} not registered with runtara-environment. Recompile it.",
                scenario_id, version
            ))
        })
    }

    /// Check that scenario is compiled and registered with runtara-environment.
    ///
    /// This is a non-blocking check. If the scenario is not compiled:
    /// - Queues compilation if not already pending
    /// - Returns `NotCompiled` error immediately (caller should retry later)
    ///
    /// This allows the trigger worker to NACK the event and retry after a delay,
    /// rather than blocking for up to 5 minutes.
    async fn ensure_compiled(
        &self,
        tenant_id: &str,
        scenario_id: &str,
        version: i32,
        _track_events: bool,
    ) -> Result<std::path::PathBuf, ExecutionError> {
        // First check if compilation is already successful
        if let Some(binary_path) = self
            .check_compilation_ready(tenant_id, scenario_id, version)
            .await?
        {
            return Ok(binary_path);
        }

        // Not compiled - queue compilation if not already pending, then return NotCompiled
        // The caller (trigger worker) will NACK and retry later
        let compilation_queued =
            if let Some(valkey_config) = crate::valkey::ValkeyConfig::from_env() {
                let redis_url = valkey_config.connection_url();

                // Check if compilation is already pending
                let is_pending = crate::workers::compilation_worker::is_compilation_pending(
                    &redis_url,
                    tenant_id,
                    scenario_id,
                    version,
                )
                .await
                .unwrap_or(false);

                if is_pending {
                    info!(
                        tenant_id = %tenant_id,
                        scenario_id = %scenario_id,
                        version = version,
                        "Compilation already pending, returning NotCompiled for retry"
                    );
                    false
                } else {
                    // Not compiled and not pending - queue it now
                    info!(
                        tenant_id = %tenant_id,
                        scenario_id = %scenario_id,
                        version = version,
                        "Scenario not compiled, queueing compilation..."
                    );

                    match crate::workers::compilation_worker::enqueue_compilation(
                        &redis_url,
                        tenant_id,
                        scenario_id,
                        version,
                    )
                    .await
                    {
                        Ok(queued) => {
                            info!(
                                tenant_id = %tenant_id,
                                scenario_id = %scenario_id,
                                version = version,
                                queued = queued,
                                "Compilation queued, returning NotCompiled for retry"
                            );
                            queued
                        }
                        Err(e) => {
                            tracing::warn!(
                                tenant_id = %tenant_id,
                                scenario_id = %scenario_id,
                                version = version,
                                error = %e,
                                "Failed to queue compilation"
                            );
                            false
                        }
                    }
                }
            } else {
                tracing::warn!(
                    tenant_id = %tenant_id,
                    scenario_id = %scenario_id,
                    version = version,
                    "Valkey not configured, cannot queue compilation"
                );
                false
            };

        // Return NotCompiled error - caller should retry later
        Err(ExecutionError::NotCompiled {
            scenario_id: scenario_id.to_string(),
            version,
            compilation_queued,
        })
    }

    /// Check if compilation is ready (successful and registered)
    /// Returns the binary path if ready, None if not ready, or error with message if failed
    async fn check_compilation_ready(
        &self,
        tenant_id: &str,
        scenario_id: &str,
        version: i32,
    ) -> Result<Option<std::path::PathBuf>, ExecutionError> {
        let compilation_record = sqlx::query!(
            r#"
            SELECT compilation_status, translated_path, registered_image_id, error_message
            FROM scenario_compilations
            WHERE tenant_id = $1 AND scenario_id = $2 AND version = $3
            "#,
            tenant_id,
            scenario_id,
            version
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            ExecutionError::DatabaseError(format!("Failed to check compilation: {}", e))
        })?;

        match compilation_record {
            Some(record)
                if record.compilation_status == "success"
                    && record.registered_image_id.is_some() =>
            {
                let binary = std::path::PathBuf::from(&record.translated_path).join("scenario");
                Ok(Some(binary))
            }
            Some(record) if record.compilation_status == "failed" => {
                // Get the error message to log prominently
                let error_msg = record
                    .error_message
                    .unwrap_or_else(|| "Unknown compilation error".to_string());
                // Log at ERROR level so it's visible in logs
                tracing::error!(
                    tenant_id = %tenant_id,
                    scenario_id = %scenario_id,
                    version = version,
                    compilation_error = %error_msg,
                    "COMPILATION FAILED - deleting record for retry"
                );
                // Delete failed record so it can be retried
                let _ = sqlx::query!(
                    "DELETE FROM scenario_compilations WHERE tenant_id = $1 AND scenario_id = $2 AND version = $3",
                    tenant_id,
                    scenario_id,
                    version
                )
                .execute(&self.pool)
                .await;
                // Return None so caller will queue a retry
                Ok(None)
            }
            _ => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // =========================================================================
    // ExecutionError Display tests
    // =========================================================================

    #[test]
    fn test_execution_error_display_scenario_not_found() {
        let error = ExecutionError::ScenarioNotFound("test-scenario".to_string());
        assert_eq!(format!("{}", error), "Scenario not found: test-scenario");
    }

    #[test]
    fn test_execution_error_display_not_compiled() {
        let error = ExecutionError::NotCompiled {
            scenario_id: "test-scenario".to_string(),
            version: 5,
            compilation_queued: true,
        };
        let display = format!("{}", error);
        assert!(display.contains("test-scenario"));
        assert!(display.contains("5"));
        assert!(display.contains("true"));
    }

    #[test]
    fn test_execution_error_display_compilation_failed() {
        let error = ExecutionError::CompilationFailed("syntax error".to_string());
        assert_eq!(format!("{}", error), "Compilation failed: syntax error");
    }

    // =========================================================================
    // ExecutionResult tests
    // =========================================================================

    #[test]
    fn test_execution_result_success() {
        let result = ExecutionResult {
            success: true,
            output: Some(json!({"result": 42})),
            error: None,
            duration_seconds: 1.5,
            max_memory_mb: Some(128.0),
            cpu_usage_ms: Some(500.0),
        };

        assert!(result.success);
        assert!(result.error.is_none());
        assert_eq!(result.output.unwrap()["result"], 42);
    }

    #[test]
    fn test_execution_result_failure() {
        let result = ExecutionResult {
            success: false,
            output: None,
            error: Some("Something went wrong".to_string()),
            duration_seconds: 0.1,
            max_memory_mb: None,
            cpu_usage_ms: None,
        };

        assert!(!result.success);
        assert!(result.output.is_none());
        assert_eq!(result.error.unwrap(), "Something went wrong");
    }
}
