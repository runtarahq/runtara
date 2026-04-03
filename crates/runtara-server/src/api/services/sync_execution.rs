//! Synchronous Scenario Execution Service
//!
//! Provides low-latency scenario execution using the RuntimeClient to communicate
//! with runtara-environment server.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use std::time::Instant;
use tracing::{info, instrument};

use crate::api::repositories::scenarios::ScenarioRepository;
use crate::api::services::scenarios::ServiceError;
use crate::metrics::MetricsService;
use crate::runtime_client::RuntimeClient;

/// Service for synchronous scenario execution (minimal latency)
pub struct SyncExecutionService {
    scenario_repo: Arc<ScenarioRepository>,
    runtime_client: Arc<RuntimeClient>,
}

impl SyncExecutionService {
    pub fn new(scenario_repo: Arc<ScenarioRepository>, runtime_client: Arc<RuntimeClient>) -> Self {
        Self {
            scenario_repo,
            runtime_client,
        }
    }

    /// Execute a scenario synchronously with minimal latency
    ///
    /// This communicates with runtara-environment server to execute the workflow
    /// and returns results immediately.
    ///
    /// # Performance
    /// - ~10-50ms overhead + execution time (via Management SDK)
    ///
    /// # Limitations
    /// - No execution history in database
    /// - Not suitable for very long-running scenarios (>30s timeout)
    #[instrument(skip(self, inputs), fields(tenant_id, scenario_id, version))]
    pub async fn execute_sync(
        &self,
        tenant_id: &str,
        scenario_id: &str,
        version: Option<i32>,
        inputs: Value,
    ) -> Result<SyncExecutionResult, ServiceError> {
        let total_start = Instant::now();

        // 1. Resolve version (use specified or get current/latest)
        let version = match version {
            Some(v) => v,
            None => self
                .scenario_repo
                .get_current_or_latest_version(tenant_id, scenario_id)
                .await
                .map_err(|e| {
                    ServiceError::DatabaseError(format!("Failed to get current version: {}", e))
                })?
                .ok_or_else(|| {
                    ServiceError::NotFound(format!("Scenario '{}' not found", scenario_id))
                })?,
        };

        if version == 0 {
            return Err(ServiceError::NotFound(format!(
                "Scenario '{}' has no versions",
                scenario_id
            )));
        }

        // 2. Validate inputs against input schema (if schema is not empty)
        let scenario = self
            .scenario_repo
            .get_by_id(tenant_id, scenario_id, Some(version))
            .await
            .map_err(|e| ServiceError::DatabaseError(format!("Failed to get scenario: {}", e)))?
            .ok_or_else(|| {
                ServiceError::NotFound(format!(
                    "Scenario '{}' version {} not found",
                    scenario_id, version
                ))
            })?;

        // Validate inputs.data against input schema (schema describes user data, not the wrapper)
        if !is_empty_schema(&scenario.input_schema) {
            let data_to_validate = inputs.get("data").cloned().unwrap_or(serde_json::json!({}));
            validate_inputs(&data_to_validate, &scenario.input_schema).map_err(|e| {
                ServiceError::ValidationError(format!("Input validation failed: {}", e))
            })?;
        }

        // 3. Check that scenario is compiled (binary exists on disk)
        // If compilation is pending in the queue, wait for it to complete
        let compilation = self
            .wait_for_compilation(tenant_id, scenario_id, version)
            .await?;

        // Note: Bundle checks removed - runtara-environment handles container execution
        // The registered_image_id check in wait_for_compilation ensures remote execution is available
        let _ = compilation; // Mark as used (translated_path not needed for remote execution)

        // 4. Get execution timeout (if set) from the executionGraph
        let execution_timeout = self
            .scenario_repo
            .get_execution_timeout(tenant_id, scenario_id, version)
            .await
            .map_err(|e| {
                ServiceError::DatabaseError(format!("Failed to get execution timeout: {}", e))
            })?;

        // 5. Get the registered image ID (UUID returned from runtara-environment)
        let image_id = self
            .scenario_repo
            .get_registered_image_id(tenant_id, scenario_id, version)
            .await
            .map_err(|e| {
                ServiceError::DatabaseError(format!("Failed to get registered image ID: {}", e))
            })?
            .ok_or_else(|| {
                ServiceError::NotFound(format!(
                    "Scenario '{}' version {} not registered with runtara-environment. Recompile it.",
                    scenario_id, version
                ))
            })?;

        // 6. Execute via RuntimeClient (runtara-environment server)
        // Sync execution doesn't support debug mode (no checkpointing)
        let execution_result = self
            .runtime_client
            .execute_sync(
                &image_id,
                tenant_id,
                scenario_id,
                None, // Auto-generate instance ID
                Some(inputs),
                execution_timeout.map(|s| s as u32),
                false,
            )
            .await;

        let total_duration = total_start.elapsed().as_secs_f64();

        // 7. Record metrics and return results
        let metrics_service = MetricsService::new(self.scenario_repo.pool().clone());

        match execution_result {
            Ok(result) => {
                let execution_duration_secs = result
                    .duration_ms
                    .map(|ms| ms as f64 / 1000.0)
                    .unwrap_or(0.0);

                // Record execution metrics (async, don't block response)
                let _ = metrics_service
                    .record_execution_completion(
                        tenant_id,
                        scenario_id,
                        version,
                        result.success,
                        execution_duration_secs,
                        None,
                    )
                    .await;

                info!(
                    tenant_id,
                    scenario_id,
                    version,
                    execution_duration_seconds = execution_duration_secs,
                    total_duration_seconds = total_duration,
                    "Synchronous execution completed"
                );

                Ok(SyncExecutionResult {
                    success: result.success,
                    outputs: result.output.unwrap_or(Value::Null),
                    error: result.error,
                    stderr: result.stderr,
                    metrics: ExecutionMetrics {
                        execution_duration_seconds: execution_duration_secs,
                        max_memory_mb: 0.0, // Not available via SDK
                        total_duration_seconds: total_duration,
                    },
                })
            }
            Err(e) => {
                let error_message = e.to_string();

                // Record failed execution metrics
                let _ = metrics_service
                    .record_execution_completion(
                        tenant_id,
                        scenario_id,
                        version,
                        false,
                        total_duration,
                        None,
                    )
                    .await;

                info!(
                    tenant_id,
                    scenario_id,
                    version,
                    error = error_message.as_str(),
                    total_duration_seconds = total_duration,
                    "Synchronous execution failed"
                );

                Ok(SyncExecutionResult {
                    success: false,
                    outputs: Value::Null,
                    error: Some(error_message),
                    stderr: None,
                    metrics: ExecutionMetrics {
                        execution_duration_seconds: 0.0,
                        max_memory_mb: 0.0,
                        total_duration_seconds: total_duration,
                    },
                })
            }
        }
    }

    /// Wait for scenario compilation to complete
    ///
    /// If compilation is already successful, returns immediately.
    /// If compilation is pending in the queue, waits for it to complete (up to 5 minutes).
    /// If not compiled and not pending, returns an error.
    async fn wait_for_compilation(
        &self,
        tenant_id: &str,
        scenario_id: &str,
        version: i32,
    ) -> Result<CompilationRecord, ServiceError> {
        // First check if compilation is already successful
        if let Some(record) = self
            .check_compilation_ready(tenant_id, scenario_id, version)
            .await?
        {
            return Ok(record);
        }

        // Check if compilation is pending in the queue - if so, wait for it
        // If not pending, queue it and wait
        if let Some(valkey_config) = crate::valkey::ValkeyConfig::from_env() {
            let redis_url = valkey_config.connection_url();

            // Check if compilation is pending
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
                    "Compilation pending, waiting for it to complete..."
                );
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
                    Ok(_) => {
                        info!(
                            tenant_id = %tenant_id,
                            scenario_id = %scenario_id,
                            version = version,
                            "Compilation queued, waiting for it to complete..."
                        );
                    }
                    Err(e) => {
                        return Err(ServiceError::ExecutionError(format!(
                            "Failed to queue compilation for scenario '{}' version {}: {}",
                            scenario_id, version, e
                        )));
                    }
                }
            }

            // Wait for compilation to complete (up to 5 minutes)
            let timeout = std::time::Duration::from_secs(300);
            let completed = crate::workers::compilation_worker::wait_for_compilation(
                &redis_url,
                tenant_id,
                scenario_id,
                version,
                timeout,
            )
            .await
            .unwrap_or(false);

            if !completed {
                return Err(ServiceError::CompilationTimeout(format!(
                    "Compilation for scenario '{}' version {} timed out after 5 minutes.",
                    scenario_id, version
                )));
            }

            // Check again if compilation succeeded
            if let Some(record) = self
                .check_compilation_ready(tenant_id, scenario_id, version)
                .await?
            {
                return Ok(record);
            }

            // Compilation completed but still not ready - something went wrong
            return Err(ServiceError::ExecutionError(format!(
                "Compilation for scenario '{}' version {} completed but binary not found.",
                scenario_id, version
            )));
        }

        // Valkey not configured - return error
        Err(ServiceError::NotFound(format!(
            "Scenario '{}' version {} not compiled and Valkey is not configured for auto-compilation.",
            scenario_id, version
        )))
    }

    /// Check if compilation is ready (successful)
    /// Returns the compilation record if ready, None if not ready
    async fn check_compilation_ready(
        &self,
        tenant_id: &str,
        scenario_id: &str,
        version: i32,
    ) -> Result<Option<CompilationRecord>, ServiceError> {
        let compilation = sqlx::query!(
            r#"
            SELECT translated_path, compilation_status, registered_image_id, error_message
            FROM scenario_compilations
            WHERE tenant_id = $1 AND scenario_id = $2 AND version = $3
            "#,
            tenant_id,
            scenario_id,
            version
        )
        .fetch_optional(self.scenario_repo.pool())
        .await
        .map_err(|e| ServiceError::DatabaseError(format!("Failed to check compilation: {}", e)))?;

        match compilation {
            Some(record)
                if record.compilation_status == "success"
                    && record.registered_image_id.is_some() =>
            {
                Ok(Some(CompilationRecord {
                    translated_path: record.translated_path,
                    compilation_status: record.compilation_status,
                }))
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
                .execute(self.scenario_repo.pool())
                .await;
                // Return None so caller will queue a retry
                Ok(None)
            }
            _ => Ok(None),
        }
    }
}

/// Internal compilation record structure
struct CompilationRecord {
    #[allow(dead_code)]
    translated_path: String,
    #[allow(dead_code)]
    compilation_status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncExecutionResult {
    pub success: bool,
    pub outputs: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Raw stderr output from the container (for debugging/logging).
    /// Separate from `error` to allow products to decide whether to show it to users.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stderr: Option<String>,
    pub metrics: ExecutionMetrics,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionMetrics {
    #[serde(rename = "executionDurationSeconds")]
    pub execution_duration_seconds: f64,
    #[serde(rename = "maxMemoryMb")]
    pub max_memory_mb: f64,
    #[serde(rename = "totalDurationSeconds")]
    pub total_duration_seconds: f64,
}

// ============================================================================
// Helper re-exports
// ============================================================================

use super::input_validation::{is_empty_schema, validate_inputs};

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // =========================================================================
    // SyncExecutionResult tests
    // =========================================================================

    #[test]
    fn test_sync_execution_result_success_serialization() {
        let result = SyncExecutionResult {
            success: true,
            outputs: json!({"result": 42, "message": "completed"}),
            error: None,
            stderr: None,
            metrics: ExecutionMetrics {
                execution_duration_seconds: 1.5,
                max_memory_mb: 128.0,
                total_duration_seconds: 1.6,
            },
        };

        let serialized = serde_json::to_value(&result).unwrap();

        assert_eq!(serialized["success"], true);
        assert_eq!(serialized["outputs"]["result"], 42);
        assert!(serialized.get("error").is_none() || serialized["error"].is_null());
        assert!(serialized.get("stderr").is_none() || serialized["stderr"].is_null());
        assert_eq!(serialized["metrics"]["executionDurationSeconds"], 1.5);
    }

    #[test]
    fn test_sync_execution_result_failure_serialization() {
        let result = SyncExecutionResult {
            success: false,
            outputs: Value::Null,
            error: Some("Execution timeout exceeded".to_string()),
            stderr: Some("Process killed after 30s".to_string()),
            metrics: ExecutionMetrics {
                execution_duration_seconds: 30.0,
                max_memory_mb: 256.0,
                total_duration_seconds: 30.5,
            },
        };

        let serialized = serde_json::to_value(&result).unwrap();

        assert_eq!(serialized["success"], false);
        assert!(serialized["outputs"].is_null());
        assert_eq!(serialized["error"], "Execution timeout exceeded");
        assert_eq!(serialized["stderr"], "Process killed after 30s");
    }

    #[test]
    fn test_sync_execution_result_deserialization() {
        let json_str = r#"{
            "success": true,
            "outputs": {"data": [1, 2, 3]},
            "metrics": {
                "executionDurationSeconds": 0.5,
                "maxMemoryMb": 64.0,
                "totalDurationSeconds": 0.6
            }
        }"#;

        let result: SyncExecutionResult = serde_json::from_str(json_str).unwrap();

        assert!(result.success);
        assert_eq!(result.outputs["data"][0], 1);
        assert!(result.error.is_none());
        assert_eq!(result.metrics.execution_duration_seconds, 0.5);
    }

    // =========================================================================
    // ExecutionMetrics tests
    // =========================================================================

    #[test]
    fn test_execution_metrics_serialization_uses_camel_case() {
        let metrics = ExecutionMetrics {
            execution_duration_seconds: 2.5,
            max_memory_mb: 512.0,
            total_duration_seconds: 3.0,
        };

        let serialized = serde_json::to_value(&metrics).unwrap();

        // Verify camelCase serialization
        assert!(serialized.get("executionDurationSeconds").is_some());
        assert!(serialized.get("maxMemoryMb").is_some());
        assert!(serialized.get("totalDurationSeconds").is_some());

        // Verify snake_case is NOT present
        assert!(serialized.get("execution_duration_seconds").is_none());
        assert!(serialized.get("max_memory_mb").is_none());
        assert!(serialized.get("total_duration_seconds").is_none());
    }

    #[test]
    fn test_execution_metrics_deserialization_from_camel_case() {
        let json_str = r#"{
            "executionDurationSeconds": 1.23,
            "maxMemoryMb": 256.5,
            "totalDurationSeconds": 1.5
        }"#;

        let metrics: ExecutionMetrics = serde_json::from_str(json_str).unwrap();

        assert!((metrics.execution_duration_seconds - 1.23).abs() < 0.001);
        assert!((metrics.max_memory_mb - 256.5).abs() < 0.001);
        assert!((metrics.total_duration_seconds - 1.5).abs() < 0.001);
    }

    #[test]
    fn test_execution_metrics_zero_values() {
        let metrics = ExecutionMetrics {
            execution_duration_seconds: 0.0,
            max_memory_mb: 0.0,
            total_duration_seconds: 0.0,
        };

        let serialized = serde_json::to_string(&metrics).unwrap();
        let deserialized: ExecutionMetrics = serde_json::from_str(&serialized).unwrap();

        assert_eq!(deserialized.execution_duration_seconds, 0.0);
        assert_eq!(deserialized.max_memory_mb, 0.0);
        assert_eq!(deserialized.total_duration_seconds, 0.0);
    }

    // =========================================================================
    // CompilationRecord tests
    // =========================================================================

    #[test]
    fn test_compilation_record_fields() {
        let record = CompilationRecord {
            translated_path: "/data/scenarios/abc/build".to_string(),
            compilation_status: "success".to_string(),
        };

        assert_eq!(record.translated_path, "/data/scenarios/abc/build");
        assert_eq!(record.compilation_status, "success");
    }
}
