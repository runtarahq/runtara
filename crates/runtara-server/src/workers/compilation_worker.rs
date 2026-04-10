//! Compilation Worker
//!
//! Background worker that processes scenario compilation requests from the queue.
//! Ensures only one compilation per scenario:version happens at a time.

use std::sync::Arc;
use std::time::{Duration, Instant};

use opentelemetry::KeyValue;
use sqlx::PgPool;
use tracing::{error, info, instrument, warn};

use crate::api::repositories::scenarios::ScenarioRepository;
use crate::api::services::compilation::CompilationService;
use crate::observability::metrics;
use crate::runtime_client::RuntimeClient;
use crate::valkey::compilation_queue::{CompilationQueue, CompilationRequest};

/// Configuration for the compilation worker
#[derive(Debug, Clone)]
pub struct CompilationWorkerConfig {
    /// Redis/Valkey connection URL
    pub redis_url: String,
    /// Timeout for blocking dequeue (seconds)
    pub dequeue_timeout_secs: u64,
    /// Connection service URL for compiled scenarios
    pub connection_service_url: Option<String>,
}

impl CompilationWorkerConfig {
    pub fn from_env(redis_url: String) -> Self {
        // With pasta --config-net, localhost URLs work in containers directly.
        let connection_service_url = std::env::var("CONNECTION_SERVICE_URL").ok();

        Self {
            redis_url,
            dequeue_timeout_secs: 5,
            connection_service_url,
        }
    }
}

/// Background worker that consumes compilation requests from the queue
#[instrument(skip(pool, runtime_client, config))]
pub async fn run(
    pool: PgPool,
    runtime_client: Option<Arc<RuntimeClient>>,
    config: CompilationWorkerConfig,
) {
    let worker_id = format!("compilation-worker-{}", uuid::Uuid::new_v4());

    info!(
        worker_id = %worker_id,
        "Starting compilation worker"
    );

    let queue = match CompilationQueue::new(config.redis_url.clone()) {
        Ok(q) => q,
        Err(e) => {
            error!(
                worker_id = %worker_id,
                error = %e,
                "Failed to create compilation queue, worker will not start"
            );
            return;
        }
    };

    // Recover any orphaned pending compilations (from previous crashes)
    match queue.recover_orphaned().await {
        Ok(count) if count > 0 => {
            info!(
                worker_id = %worker_id,
                recovered_count = count,
                "Recovered orphaned pending compilations"
            );
        }
        Ok(_) => {}
        Err(e) => {
            error!(
                worker_id = %worker_id,
                error = %e,
                "Failed to recover orphaned compilations"
            );
        }
    }

    let repository = Arc::new(ScenarioRepository::new(pool.clone()));
    let compilation_service = CompilationService::new(
        repository.clone(),
        config.connection_service_url.clone(),
        runtime_client,
    );

    let dequeue_timeout = Duration::from_secs(config.dequeue_timeout_secs);

    loop {
        // Dequeue next compilation request (blocking with timeout)
        match queue.dequeue(dequeue_timeout).await {
            Ok(Some(request)) => {
                info!(
                    worker_id = %worker_id,
                    tenant_id = %request.tenant_id,
                    scenario_id = %request.scenario_id,
                    version = request.version,
                    "Processing compilation request"
                );

                // Check if already compiled (skip if successful compilation exists)
                let should_compile = match check_compilation_status(
                    &pool,
                    &request.tenant_id,
                    &request.scenario_id,
                    request.version,
                )
                .await
                {
                    Ok(CompilationStatus::Success) => {
                        info!(
                            tenant_id = %request.tenant_id,
                            scenario_id = %request.scenario_id,
                            version = request.version,
                            "Scenario already compiled, skipping"
                        );
                        false
                    }
                    Ok(CompilationStatus::NotCompiled) | Ok(CompilationStatus::Failed) => true,
                    Err(e) => {
                        error!(
                            tenant_id = %request.tenant_id,
                            scenario_id = %request.scenario_id,
                            version = request.version,
                            error = %e,
                            "Failed to check compilation status, will attempt compilation"
                        );
                        true
                    }
                };

                if should_compile {
                    let compile_start = Instant::now();
                    let attributes = [
                        KeyValue::new("tenant_id", request.tenant_id.clone()),
                        KeyValue::new("scenario_id", request.scenario_id.clone()),
                    ];

                    // Track active compilations
                    if let Some(m) = metrics() {
                        m.compilations_active.add(1, &attributes);
                    }

                    // Perform compilation (target determined by RUNTARA_COMPILE_TARGET env var)
                    let compile_result = compilation_service
                        .compile_scenario(&request.tenant_id, &request.scenario_id, request.version)
                        .await;

                    // Record metrics
                    let duration = compile_start.elapsed().as_secs_f64();
                    let success = compile_result.is_ok();

                    if let Some(m) = metrics() {
                        let result_attrs = [
                            KeyValue::new("tenant_id", request.tenant_id.clone()),
                            KeyValue::new("scenario_id", request.scenario_id.clone()),
                            KeyValue::new("status", if success { "success" } else { "failed" }),
                        ];
                        m.compilations_total.add(1, &result_attrs);
                        m.compilation_duration.record(duration, &attributes);
                        m.compilations_active.add(-1, &attributes);
                    }

                    match compile_result {
                        Ok(result) => {
                            info!(
                                tenant_id = %request.tenant_id,
                                scenario_id = %request.scenario_id,
                                version = request.version,
                                binary_size = result.binary_size,
                                image_id = ?result.image_id,
                                duration_secs = duration,
                                "Compilation completed successfully"
                            );
                        }
                        Err(e) => {
                            error!(
                                tenant_id = %request.tenant_id,
                                scenario_id = %request.scenario_id,
                                version = request.version,
                                error = %e,
                                duration_secs = duration,
                                "Compilation failed"
                            );
                            // Record the failure in database
                            if let Err(db_err) = record_compilation_failure(
                                &pool,
                                &request.tenant_id,
                                &request.scenario_id,
                                request.version,
                                &e.to_string(),
                            )
                            .await
                            {
                                error!(error = %db_err, "Failed to record compilation failure");
                            }
                        }
                    }
                }

                // Mark as complete (removes from pending set)
                if let Err(e) = queue.complete(&request).await {
                    error!(
                        tenant_id = %request.tenant_id,
                        scenario_id = %request.scenario_id,
                        version = request.version,
                        error = %e,
                        "Failed to mark compilation as complete"
                    );
                }
            }
            Ok(None) => {
                // Timeout - no requests in queue, continue polling
            }
            Err(e) => {
                error!(error = %e, "Error dequeuing compilation request");
                // Wait before retrying to avoid tight loop on persistent errors
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    }
}

/// Compilation status from database
enum CompilationStatus {
    Success,
    Failed,
    NotCompiled,
}

/// Check if a scenario version is already compiled
async fn check_compilation_status(
    pool: &PgPool,
    tenant_id: &str,
    scenario_id: &str,
    version: i32,
) -> Result<CompilationStatus, sqlx::Error> {
    let result = sqlx::query!(
        r#"
        SELECT compilation_status, registered_image_id
        FROM scenario_compilations
        WHERE tenant_id = $1 AND scenario_id = $2 AND version = $3
        "#,
        tenant_id,
        scenario_id,
        version
    )
    .fetch_optional(pool)
    .await?;

    match result {
        Some(record) => {
            if record.compilation_status == "success" && record.registered_image_id.is_some() {
                Ok(CompilationStatus::Success)
            } else if record.compilation_status == "failed" {
                Ok(CompilationStatus::Failed)
            } else {
                // Partial compilation (compiled but not registered) - retry
                Ok(CompilationStatus::NotCompiled)
            }
        }
        None => Ok(CompilationStatus::NotCompiled),
    }
}

/// Record a compilation failure in the database
async fn record_compilation_failure(
    pool: &PgPool,
    tenant_id: &str,
    scenario_id: &str,
    version: i32,
    error_message: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"
        INSERT INTO scenario_compilations
            (tenant_id, scenario_id, version, compilation_status, translated_path, compiled_at, error_message, runtara_version)
        VALUES ($1, $2, $3, 'failed', '', NOW(), $4, $5)
        ON CONFLICT (tenant_id, scenario_id, version)
        DO UPDATE SET
            compilation_status = 'failed',
            compiled_at = NOW(),
            error_message = $4,
            runtara_version = $5
        "#,
        tenant_id,
        scenario_id,
        version,
        error_message,
        env!("BUILD_VERSION")
    )
    .execute(pool)
    .await?;

    warn!(
        tenant_id = %tenant_id,
        scenario_id = %scenario_id,
        version = version,
        error = %error_message,
        "Recorded compilation failure"
    );

    Ok(())
}

/// Enqueue a scenario for compilation
///
/// This is the main entry point for scheduling compilations.
/// Returns `true` if the request was queued, `false` if already pending.
pub async fn enqueue_compilation(
    redis_url: &str,
    tenant_id: &str,
    scenario_id: &str,
    version: i32,
) -> Result<bool, crate::valkey::compilation_queue::CompilationQueueError> {
    let queue = CompilationQueue::new(redis_url.to_string())?;
    let request = CompilationRequest::new(tenant_id.to_string(), scenario_id.to_string(), version);
    queue.enqueue(&request).await
}

/// Check if a compilation is pending (in queue or being processed)
pub async fn is_compilation_pending(
    redis_url: &str,
    tenant_id: &str,
    scenario_id: &str,
    version: i32,
) -> Result<bool, crate::valkey::compilation_queue::CompilationQueueError> {
    let queue = CompilationQueue::new(redis_url.to_string())?;
    let request = CompilationRequest::new(tenant_id.to_string(), scenario_id.to_string(), version);
    queue.is_pending(&request).await
}

/// Wait for a compilation to complete
///
/// Returns `true` if compilation completed, `false` if timeout.
pub async fn wait_for_compilation(
    redis_url: &str,
    tenant_id: &str,
    scenario_id: &str,
    version: i32,
    timeout: Duration,
) -> Result<bool, crate::valkey::compilation_queue::CompilationQueueError> {
    let queue = CompilationQueue::new(redis_url.to_string())?;
    let request = CompilationRequest::new(tenant_id.to_string(), scenario_id.to_string(), version);
    let poll_interval = Duration::from_millis(100);
    queue
        .wait_for_completion(&request, timeout, poll_interval)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // CompilationWorkerConfig tests
    // =========================================================================

    #[test]
    fn test_compilation_worker_config_from_env() {
        // Test with a mock redis URL (from_env doesn't actually connect)
        let config = CompilationWorkerConfig::from_env("redis://localhost:6379".to_string());

        assert_eq!(config.redis_url, "redis://localhost:6379");
        assert_eq!(config.dequeue_timeout_secs, 5);
        // connection_service_url depends on env var - just check it's loaded
    }

    #[test]
    fn test_compilation_worker_config_debug_format() {
        let config = CompilationWorkerConfig {
            redis_url: "redis://test:6379".to_string(),
            dequeue_timeout_secs: 10,
            connection_service_url: Some("http://connection-service:8080".to_string()),
        };

        let debug_str = format!("{:?}", config);
        assert!(debug_str.contains("redis_url"));
        assert!(debug_str.contains("redis://test:6379"));
        assert!(debug_str.contains("dequeue_timeout_secs"));
        assert!(debug_str.contains("10"));
        assert!(debug_str.contains("connection_service_url"));
    }

    #[test]
    fn test_compilation_worker_config_clone() {
        let config = CompilationWorkerConfig {
            redis_url: "redis://primary:6379".to_string(),
            dequeue_timeout_secs: 15,
            connection_service_url: None,
        };

        let cloned = config.clone();
        assert_eq!(cloned.redis_url, config.redis_url);
        assert_eq!(cloned.dequeue_timeout_secs, config.dequeue_timeout_secs);
        assert_eq!(cloned.connection_service_url, config.connection_service_url);
    }

    #[test]
    fn test_compilation_worker_config_with_connection_service() {
        let config = CompilationWorkerConfig {
            redis_url: "redis://localhost:6379".to_string(),
            dequeue_timeout_secs: 5,
            connection_service_url: Some("http://connections.internal:3000".to_string()),
        };

        assert!(config.connection_service_url.is_some());
        assert_eq!(
            config.connection_service_url.unwrap(),
            "http://connections.internal:3000"
        );
    }

    #[test]
    fn test_compilation_worker_config_without_connection_service() {
        let config = CompilationWorkerConfig {
            redis_url: "redis://localhost:6379".to_string(),
            dequeue_timeout_secs: 5,
            connection_service_url: None,
        };

        assert!(config.connection_service_url.is_none());
    }

    // =========================================================================
    // CompilationStatus tests
    // =========================================================================

    #[test]
    fn test_compilation_status_success_variant() {
        let status = CompilationStatus::Success;
        assert!(matches!(status, CompilationStatus::Success));
    }

    #[test]
    fn test_compilation_status_failed_variant() {
        let status = CompilationStatus::Failed;
        assert!(matches!(status, CompilationStatus::Failed));
    }

    #[test]
    fn test_compilation_status_not_compiled_variant() {
        let status = CompilationStatus::NotCompiled;
        assert!(matches!(status, CompilationStatus::NotCompiled));
    }

    #[test]
    fn test_compilation_status_exhaustive_match() {
        // This test verifies all variants can be matched
        // Compiler will fail if enum changes without updating this
        let statuses = [
            CompilationStatus::Success,
            CompilationStatus::Failed,
            CompilationStatus::NotCompiled,
        ];

        for status in statuses {
            match status {
                CompilationStatus::Success => {}
                CompilationStatus::Failed => {}
                CompilationStatus::NotCompiled => {}
            }
        }
    }
}
