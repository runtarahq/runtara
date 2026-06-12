//! Compilation Worker
//!
//! Background worker that processes workflow compilation requests from the queue.
//! Ensures only one compilation per workflow:version happens at a time.

use std::sync::Arc;
use std::time::{Duration, Instant};

use opentelemetry::KeyValue;
use sqlx::PgPool;
use tracing::{error, info, instrument, warn};

use crate::api::repositories::workflows::WorkflowRepository;
use crate::api::services::compilation::{
    CompilationService, direct_compilation_settings_from_config,
};
use crate::observability::metrics;
use crate::runtime_client::RuntimeClient;
use crate::shutdown::ShutdownSignal;
use crate::valkey::compilation_queue::{CompilationQueue, CompilationRequest};

/// Configuration for the compilation worker
#[derive(Debug, Clone)]
pub struct CompilationWorkerConfig {
    /// Redis/Valkey connection URL
    pub redis_url: String,
    /// Timeout for blocking dequeue (seconds)
    pub dequeue_timeout_secs: u64,
    /// Connection service URL for compiled workflows
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
#[instrument(skip(pool, runtime_client, agent_catalog, config, shutdown))]
pub async fn run(
    pool: PgPool,
    runtime_client: Option<Arc<RuntimeClient>>,
    agent_catalog: Option<Arc<runtara_dsl::agent_meta::AgentCatalog>>,
    config: CompilationWorkerConfig,
    shutdown: ShutdownSignal,
) {
    let worker_id = format!("compilation-worker-{}", uuid::Uuid::new_v4());

    info!(
        worker_id = %worker_id,
        "Starting compilation worker"
    );

    // Blocking BLPOP consumer — must not ride the shared manager. See
    // `crate::valkey::dedicated_manager_for_blocking_consumer` for the rule.
    let manager = match crate::valkey::dedicated_manager_for_blocking_consumer(
        config.redis_url.as_str(),
        &worker_id,
    )
    .await
    {
        Ok(m) => m,
        Err(e) => {
            error!(
                worker_id = %worker_id,
                error = %e,
                "Failed to initialize Redis connection manager, worker will not start"
            );
            return;
        }
    };
    let queue = CompilationQueue::new(manager);

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

    let repository = Arc::new(WorkflowRepository::new(pool.clone()));
    let mut compilation_service = CompilationService::new(
        repository.clone(),
        config.connection_service_url.clone(),
        runtime_client,
    )
    .with_direct_compilation(direct_compilation_settings_from_config());
    if let Some(catalog) = agent_catalog {
        compilation_service = compilation_service.with_agent_catalog(catalog);
    }

    // Reuse the shared (non-blocking) manager for progress writes — they're
    // small HSET/EXPIRE ops, not blocking commands, so they belong on the
    // shared manager rather than the dedicated BLPOP one.
    let progress_manager = match crate::valkey::get_or_create_manager(&config.redis_url).await {
        Ok(m) => Some(m),
        Err(e) => {
            warn!(
                worker_id = %worker_id,
                error = %e,
                "Failed to obtain shared Redis manager for progress reporting; compilations will run without progress events"
            );
            None
        }
    };
    if let Some(m) = progress_manager.clone() {
        compilation_service = compilation_service.with_redis_manager(m);
    }

    let dequeue_timeout = Duration::from_secs(config.dequeue_timeout_secs);

    loop {
        if shutdown.is_shutting_down() {
            info!(worker_id = %worker_id, "Compilation worker exiting on shutdown signal");
            return;
        }

        // Dequeue next compilation request (blocking with timeout)
        match queue.dequeue(dequeue_timeout).await {
            Ok(Some(request)) => {
                info!(
                    worker_id = %worker_id,
                    tenant_id = %request.tenant_id,
                    workflow_id = %request.workflow_id,
                    version = request.version,
                    force_recompile = request.force_recompile,
                    "Processing compilation request"
                );

                // Direct WASM is the only compile path now, so cache decisions
                // are always deferred to CompilationService (which accounts for
                // the desired compiler mode) instead of an older source-only
                // check here. The service short-circuits when a fresh image
                // already exists, so the worker always hands the request off.
                {
                    let compile_start = Instant::now();
                    let attributes = [
                        KeyValue::new("tenant_id", request.tenant_id.clone()),
                        KeyValue::new("workflow_id", request.workflow_id.clone()),
                    ];

                    // Track active compilations
                    if let Some(m) = metrics() {
                        m.compilations_active.add(1, &attributes);
                    }

                    // Perform compilation (target determined by RUNTARA_COMPILE_TARGET env var)
                    let compile_result = compilation_service
                        .compile_workflow(
                            &request.tenant_id,
                            &request.workflow_id,
                            request.version,
                            request.force_recompile,
                        )
                        .await;

                    // Record metrics
                    let duration = compile_start.elapsed().as_secs_f64();
                    let success = compile_result.is_ok();

                    if let Some(m) = metrics() {
                        let result_attrs = [
                            KeyValue::new("tenant_id", request.tenant_id.clone()),
                            KeyValue::new("workflow_id", request.workflow_id.clone()),
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
                                workflow_id = %request.workflow_id,
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
                                workflow_id = %request.workflow_id,
                                version = request.version,
                                error = %e,
                                duration_secs = duration,
                                "Compilation failed"
                            );
                            // Record the failure in database
                            if let Err(db_err) = record_compilation_failure(
                                &pool,
                                &request.tenant_id,
                                &request.workflow_id,
                                request.version,
                                &e.to_string(),
                            )
                            .await
                            {
                                error!(error = %db_err, "Failed to record compilation failure");
                            }
                            // Terminal state (failed) is now in the DB.
                            // Clear the Redis progress entry so polling
                            // clients fall through and pick up the failure.
                            if let Some(m) = &progress_manager {
                                crate::valkey::compilation_progress::ProgressReporter::new(
                                    m.clone(),
                                    &request.tenant_id,
                                    &request.workflow_id,
                                    request.version,
                                )
                                .clear()
                                .await;
                            }
                        }
                    }
                }

                // Mark as complete (removes from pending set)
                if let Err(e) = queue.complete(&request).await {
                    error!(
                        tenant_id = %request.tenant_id,
                        workflow_id = %request.workflow_id,
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

/// Record a compilation failure in the database
async fn record_compilation_failure(
    pool: &PgPool,
    tenant_id: &str,
    workflow_id: &str,
    version: i32,
    error_message: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"
        INSERT INTO workflow_compilations
            (tenant_id, workflow_id, version, compilation_status, translated_path, compiled_at, error_message, runtara_version)
        VALUES ($1, $2, $3, 'failed', '', NOW(), $4, $5)
        ON CONFLICT (tenant_id, workflow_id, version)
        DO UPDATE SET
            compilation_status = 'failed',
            compiled_at = NOW(),
            error_message = $4,
            runtara_version = $5
        "#,
        tenant_id,
        workflow_id,
        version,
        error_message,
        env!("BUILD_VERSION")
    )
    .execute(pool)
    .await?;

    warn!(
        tenant_id = %tenant_id,
        workflow_id = %workflow_id,
        version = version,
        error = %error_message,
        "Recorded compilation failure"
    );

    Ok(())
}

/// Enqueue a workflow for compilation
///
/// This is the main entry point for scheduling compilations.
/// Returns `true` if the request was queued, `false` if already pending.
pub async fn enqueue_compilation(
    redis_url: &str,
    tenant_id: &str,
    workflow_id: &str,
    version: i32,
    force_recompile: bool,
) -> Result<bool, crate::valkey::compilation_queue::CompilationQueueError> {
    let queue = open_shared_queue(redis_url).await?;
    let request = CompilationRequest::new_with_force(
        tenant_id.to_string(),
        workflow_id.to_string(),
        version,
        force_recompile,
    );
    queue.enqueue(&request).await
}

/// Check if a compilation is pending (in queue or being processed)
pub async fn is_compilation_pending(
    redis_url: &str,
    tenant_id: &str,
    workflow_id: &str,
    version: i32,
) -> Result<bool, crate::valkey::compilation_queue::CompilationQueueError> {
    let queue = open_shared_queue(redis_url).await?;
    let request = CompilationRequest::new(tenant_id.to_string(), workflow_id.to_string(), version);
    queue.is_pending(&request).await
}

/// Wait for a compilation to complete
///
/// Returns `true` if compilation completed, `false` if timeout.
pub async fn wait_for_compilation(
    redis_url: &str,
    tenant_id: &str,
    workflow_id: &str,
    version: i32,
    timeout: Duration,
) -> Result<bool, crate::valkey::compilation_queue::CompilationQueueError> {
    let queue = open_shared_queue(redis_url).await?;
    let request = CompilationRequest::new(tenant_id.to_string(), workflow_id.to_string(), version);
    let poll_interval = Duration::from_millis(100);
    queue
        .wait_for_completion(&request, timeout, poll_interval)
        .await
}

/// Build a CompilationQueue backed by the shared, process-wide connection
/// manager so each utility call reuses an existing pooled connection
/// instead of opening a new TCP socket.
async fn open_shared_queue(
    redis_url: &str,
) -> Result<CompilationQueue, crate::valkey::compilation_queue::CompilationQueueError> {
    let manager = crate::valkey::get_or_create_manager(redis_url)
        .await
        .map_err(|e| {
            crate::valkey::compilation_queue::CompilationQueueError::ConnectionError(e.to_string())
        })?;
    Ok(CompilationQueue::new(manager))
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
}
