//! Compilation Worker
//!
//! Background worker that processes workflow compilation requests from the queue.
//! Ensures only one compilation per workflow:version happens at a time.

use std::sync::Arc;
use std::time::{Duration, Instant};

use opentelemetry::KeyValue;
use sqlx::PgPool;
use tracing::{error, info, instrument, warn};

use crate::api::repositories::workflows::{WorkflowRepository, workflow_definition_checksum};
use crate::api::services::compilation::{
    CompilationService, ServiceError as CompilationServiceError,
    direct_compilation_settings_from_config,
};
use crate::observability::metrics;
use crate::product_events::{ActorType, EventSource, EventType, ProductEvent, ProductEventSink};
use crate::runtime_client::RuntimeClient;
use crate::shutdown::ShutdownSignal;
use crate::valkey::compilation_queue::{CompilationQueue, CompilationRequest};

/// Configuration for the compilation worker
#[derive(Clone)]
pub struct CompilationWorkerConfig {
    /// Redis/Valkey connection URL
    pub redis_url: String,
    /// Timeout for blocking dequeue (seconds)
    pub dequeue_timeout_secs: u64,
    /// Connection service URL for compiled workflows
    pub connection_service_url: Option<String>,
}

/// Manual `Debug` so logging this config can never leak the password embedded
/// in the redis URL.
impl std::fmt::Debug for CompilationWorkerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompilationWorkerConfig")
            .field(
                "redis_url",
                &crate::valkey::redact_credentials(&self.redis_url),
            )
            .field("dequeue_timeout_secs", &self.dequeue_timeout_secs)
            .field("connection_service_url", &self.connection_service_url)
            .finish()
    }
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
#[instrument(skip(pool, runtime_client, agent_catalog, config, shutdown, events))]
pub async fn run(
    pool: PgPool,
    runtime_client: Option<Arc<RuntimeClient>>,
    agent_catalog: Option<Arc<runtara_dsl::agent_meta::AgentCatalog>>,
    config: CompilationWorkerConfig,
    shutdown: ShutdownSignal,
    events: ProductEventSink,
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

                    // Track active compilations. No labels: like the duration
                    // histogram below, a per-(tenant, workflow) gauge would grow
                    // one series per combination without bound.
                    if let Some(m) = metrics() {
                        m.compilations_active.add(1, &[]);
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

                    // Product analytics: the worker is the single emit point for
                    // `workflow.compiled`. The enqueuer optionally supplied an attributed event
                    // (caller + surface); otherwise we emit a no-user, Worker-source default.
                    // Either way it lands exactly once, even if the requesting handler timed out.
                    let mut event = request.product_event.clone().unwrap_or_else(|| {
                        ProductEvent::new(EventType::WorkflowCompiled)
                            .no_user_actor("compilation_worker", ActorType::System)
                            .resource(&request.workflow_id, "workflow")
                            .source(EventSource::Worker)
                    });
                    event.properties = serde_json::json!({ "success": success });
                    event.occurred_at = chrono::Utc::now();
                    events.emit(event);

                    if let Some(m) = metrics() {
                        let status = if success { "success" } else { "failed" };
                        let result_attrs = [
                            KeyValue::new("tenant_id", request.tenant_id.clone()),
                            KeyValue::new("workflow_id", request.workflow_id.clone()),
                            KeyValue::new("status", status),
                        ];
                        m.compilations_total.add(1, &result_attrs);
                        // Duration is a histogram: drop tenant_id/workflow_id so
                        // its buckets don't multiply per workflow and tenant.
                        m.compilation_duration
                            .record(duration, &[KeyValue::new("status", status)]);
                        m.compilations_active.add(-1, &[]);
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
                            // A graph that cannot compile as authored is the
                            // author's problem, not a fault to alert on, so it
                            // is recorded at a lower level than a real failure.
                            if matches!(e, CompilationServiceError::WorkflowAuthoringError(_)) {
                                warn!(
                                    tenant_id = %request.tenant_id,
                                    workflow_id = %request.workflow_id,
                                    version = request.version,
                                    error = %e,
                                    duration_secs = duration,
                                    "Workflow cannot be compiled as authored"
                                );
                            } else {
                                error!(
                                    tenant_id = %request.tenant_id,
                                    workflow_id = %request.workflow_id,
                                    version = request.version,
                                    error = %e,
                                    duration_secs = duration,
                                    "Compilation failed"
                                );
                            }
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
    // Stamp the checksum of the definition that failed. Readers compare it
    // against the definition currently stored to tell a failure that will
    // recur (source unchanged) from a stale one worth retrying.
    let source_checksum: Option<String> = sqlx::query_scalar::<_, serde_json::Value>(
        r#"
        SELECT definition
        FROM workflow_definitions
        WHERE tenant_id = $1 AND workflow_id = $2 AND version = $3 AND deleted_at IS NULL
        "#,
    )
    .bind(tenant_id)
    .bind(workflow_id)
    .bind(version)
    .fetch_optional(pool)
    .await?
    .as_ref()
    .map(workflow_definition_checksum);

    sqlx::query(
        r#"
        INSERT INTO workflow_compilations
            (tenant_id, workflow_id, version, compilation_status, translated_path, compiled_at, error_message, runtara_version, source_checksum)
        VALUES ($1, $2, $3, 'failed', '', NOW(), $4, $5, $6)
        ON CONFLICT (tenant_id, workflow_id, version)
        DO UPDATE SET
            compilation_status = 'failed',
            compiled_at = NOW(),
            error_message = $4,
            runtara_version = $5,
            source_checksum = $6
        "#,
    )
    .bind(tenant_id)
    .bind(workflow_id)
    .bind(version)
    .bind(error_message)
    .bind(env!("BUILD_VERSION"))
    .bind(source_checksum.as_deref())
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
    enqueue_compilation_inner(
        redis_url,
        tenant_id,
        workflow_id,
        version,
        force_recompile,
        None,
    )
    .await
}

/// Like [`enqueue_compilation`], but hands the worker a pre-built, attributed
/// `workflow.compiled` event to emit on completion (the worker fills in `success` /
/// `occurred_at`). Use from a handler that has the caller/surface context.
pub async fn enqueue_compilation_with_event(
    redis_url: &str,
    tenant_id: &str,
    workflow_id: &str,
    version: i32,
    force_recompile: bool,
    product_event: ProductEvent,
) -> Result<bool, crate::valkey::compilation_queue::CompilationQueueError> {
    enqueue_compilation_inner(
        redis_url,
        tenant_id,
        workflow_id,
        version,
        force_recompile,
        Some(product_event),
    )
    .await
}

async fn enqueue_compilation_inner(
    redis_url: &str,
    tenant_id: &str,
    workflow_id: &str,
    version: i32,
    force_recompile: bool,
    product_event: Option<ProductEvent>,
) -> Result<bool, crate::valkey::compilation_queue::CompilationQueueError> {
    let queue = open_shared_queue(redis_url).await?;
    let request = CompilationRequest::new_with_force(
        tenant_id.to_string(),
        workflow_id.to_string(),
        version,
        force_recompile,
    )
    .with_product_event(product_event);
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
    fn test_compilation_worker_config_debug_redacts_credentials() {
        let config = CompilationWorkerConfig {
            redis_url: "rediss://app:s3cret%40pw@valkey.internal:6390".to_string(),
            dequeue_timeout_secs: 10,
            connection_service_url: None,
        };

        let debug_str = format!("{:?}", config);
        assert!(
            !debug_str.contains("s3cret"),
            "password leaked: {debug_str}"
        );
        assert!(!debug_str.contains("app:"), "username leaked: {debug_str}");
        assert!(
            debug_str.contains("rediss://***@valkey.internal:6390"),
            "host/port must stay visible for debugging: {debug_str}"
        );
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
