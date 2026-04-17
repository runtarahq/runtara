//! Trigger Worker
//!
//! Consumes trigger events from Valkey streams and executes scenarios.
//! This replaces the DB-polling native_worker with stream-based execution.

use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use opentelemetry::KeyValue;
use sqlx::PgPool;
use tracing::{error, info, instrument, warn};
use uuid::Uuid;

use crate::api::dto::trigger_event::TriggerEvent;
use crate::api::repositories::scenarios::ScenarioRepository;
use crate::observability::metrics;
use crate::runtime_client::RuntimeClient;
use crate::shutdown::ShutdownSignal;
use crate::types::CancellationHandle;
use crate::valkey::ValkeyConfig;
use crate::valkey::client::ValkeyClient;
use crate::valkey::stream::StreamConsumer;
use crate::workers::execution_engine::{ExecutionEngine, ExecutionError};

/// Result of processing a trigger event
#[derive(Debug)]
enum ProcessResult {
    /// Event processed successfully - ACK it
    Success,
    /// Event failed permanently - ACK it to prevent infinite retries
    PermanentFailure(String),
    /// Event should be retried later - DON'T ACK (stays in PEL)
    RetryLater(String),
}

/// Configuration for the trigger worker
#[derive(Debug, Clone)]
pub struct TriggerWorkerConfig {
    /// Tenant ID (from TENANT_ID env var)
    pub tenant_id: String,
    /// Maximum events to read per batch
    pub batch_size: usize,
    /// Block timeout in milliseconds when waiting for events
    pub block_timeout_ms: usize,
    /// Minimum idle time (ms) before pending events can be reclaimed for retry.
    /// Events that fail with NotCompiled will be retried after this delay.
    /// Default: 10000 (10 seconds)
    pub pending_retry_delay_ms: u64,
    /// Maximum pending events to claim per iteration
    /// Default: 5
    pub pending_batch_size: usize,
    /// Maximum number of retries before giving up on an event
    /// Default: 5 (5 retries * 10s = ~50 seconds)
    pub max_retries: u64,
}

impl Default for TriggerWorkerConfig {
    fn default() -> Self {
        Self {
            tenant_id: std::env::var("TENANT_ID").unwrap_or_else(|_| "default".to_string()),
            batch_size: 10,
            block_timeout_ms: 5000,
            pending_retry_delay_ms: std::env::var("TRIGGER_PENDING_RETRY_DELAY_MS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(10_000), // 10 seconds
            pending_batch_size: std::env::var("TRIGGER_PENDING_BATCH_SIZE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(5),
            max_retries: std::env::var("TRIGGER_MAX_RETRIES")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(5), // 5 retries = ~50 seconds at 10s interval
        }
    }
}

/// Background worker that consumes trigger events from Valkey streams
/// and executes scenarios using the ExecutionEngine.
#[instrument(skip(pool, running_executions, runtime_client, valkey_config, shutdown))]
pub async fn run(
    pool: PgPool,
    running_executions: Arc<DashMap<Uuid, CancellationHandle>>,
    runtime_client: Option<Arc<RuntimeClient>>,
    valkey_config: ValkeyConfig,
    worker_config: TriggerWorkerConfig,
    shutdown: ShutdownSignal,
) {
    let worker_id = format!("trigger-worker-{}", Uuid::new_v4());
    let tenant_id = worker_config.tenant_id.clone();

    info!(
        worker_id = %worker_id,
        tenant_id = %tenant_id,
        "Starting trigger worker"
    );

    // Connect to Valkey
    let client = match ValkeyClient::new(valkey_config.clone()).await {
        Ok(client) => client,
        Err(e) => {
            error!(error = %e, "Failed to connect to Valkey, trigger worker will not start");
            return;
        }
    };

    // Create stream consumer for trigger events
    let connection = client.get_connection();
    let stream_key = valkey_config.trigger_stream_key(&tenant_id);

    let mut consumer = StreamConsumer::new(
        connection,
        stream_key.clone(),
        valkey_config.trigger_consumer_group.clone(),
        worker_id.clone(),
    );

    // Initialize consumer group
    if let Err(e) = consumer.initialize_consumer_group().await {
        error!(error = %e, "Failed to initialize consumer group, trigger worker will not start");
        return;
    }

    info!(
        worker_id = %worker_id,
        stream_key = %stream_key,
        consumer_group = %valkey_config.trigger_consumer_group,
        pending_retry_delay_ms = worker_config.pending_retry_delay_ms,
        max_retries = worker_config.max_retries,
        "Trigger worker listening for events"
    );

    // Create execution engine (wrapped in Arc for sharing across spawned tasks)
    let scenario_repo = Arc::new(ScenarioRepository::new(pool.clone()));
    let engine = Arc::new(ExecutionEngine::new(
        pool.clone(),
        scenario_repo,
        runtime_client,
        None, // trigger_stream not needed for the trigger worker
        Some(running_executions.clone()),
    ));

    // Track the start ID for XAUTOCLAIM pagination
    let mut autoclaim_start_id = "0-0".to_string();

    // Main event processing loop - two phases:
    // 1. Process pending events (retries) using XAUTOCLAIM
    // 2. Process new events using XREADGROUP
    loop {
        if shutdown.is_shutting_down() {
            info!(worker_id = %worker_id, "Trigger worker exiting on shutdown signal");
            return;
        }

        // PHASE 1: Process pending events (retries)
        // These are events that weren't ACKed (e.g., NotCompiled errors)
        match consumer
            .claim_pending_events(
                worker_config.pending_retry_delay_ms,
                worker_config.pending_batch_size,
                &autoclaim_start_id,
            )
            .await
        {
            Ok((pending_events, next_start_id)) => {
                autoclaim_start_id = next_start_id;

                for (entry_id, valkey_event) in pending_events {
                    process_event(
                        &mut consumer,
                        &engine,
                        &entry_id,
                        &valkey_event,
                        &running_executions,
                        worker_config.max_retries,
                        true, // is_retry
                    )
                    .await;
                }
            }
            Err(e) => {
                // Log but continue to process new events
                warn!(error = %e, "Failed to claim pending events");
            }
        }

        // PHASE 2: Process new events
        match consumer
            .read_events(worker_config.block_timeout_ms, worker_config.batch_size)
            .await
        {
            Ok(events) => {
                for (entry_id, valkey_event) in events {
                    process_event(
                        &mut consumer,
                        &engine,
                        &entry_id,
                        &valkey_event,
                        &running_executions,
                        worker_config.max_retries,
                        false, // is_retry
                    )
                    .await;
                }
            }
            Err(e) => {
                error!(error = %e, "Error reading from Valkey stream, retrying in 5 seconds");
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    }
}

/// Process a single event from the stream
async fn process_event(
    consumer: &mut StreamConsumer,
    engine: &Arc<ExecutionEngine>,
    entry_id: &str,
    valkey_event: &crate::valkey::events::ValkeyEvent,
    _running_executions: &Arc<DashMap<Uuid, CancellationHandle>>,
    max_retries: u64,
    is_retry: bool,
) {
    // Parse TriggerEvent from the stream data
    let trigger_event = match parse_trigger_event(valkey_event) {
        Ok(event) => event,
        Err(e) => {
            warn!(
                entry_id = %entry_id,
                error = %e,
                "Failed to parse trigger event, acknowledging and skipping"
            );
            // Acknowledge malformed events to prevent reprocessing
            if let Err(ack_err) = consumer.acknowledge_event(entry_id).await {
                error!(entry_id = %entry_id, error = %ack_err, "Failed to ACK malformed event");
            }
            return;
        }
    };

    let retry_suffix = if is_retry { " (retry)" } else { "" };
    info!(
        entry_id = %entry_id,
        instance_id = %trigger_event.instance_id,
        scenario_id = %trigger_event.scenario_id,
        trigger_type = %trigger_event.trigger_type(),
        is_retry = is_retry,
        "Processing trigger event{}",
        retry_suffix
    );

    // Track processing time
    let process_start = Instant::now();
    let attributes = [
        KeyValue::new("tenant_id", trigger_event.tenant_id.clone()),
        KeyValue::new("scenario_id", trigger_event.scenario_id.clone()),
        KeyValue::new("trigger_type", trigger_event.trigger_type().to_string()),
    ];

    // Process the trigger event
    let process_result = process_trigger_event(engine.clone(), &trigger_event).await;

    // Record metrics
    let duration = process_start.elapsed().as_secs_f64();
    if let Some(m) = metrics() {
        m.trigger_events_total.add(1, &attributes);
        m.trigger_processing_duration.record(duration, &attributes);
        match &process_result {
            ProcessResult::PermanentFailure(_) => {
                m.trigger_events_failed.add(1, &attributes);
            }
            ProcessResult::RetryLater(_) => {
                // Don't count as failed yet - it will be retried
            }
            ProcessResult::Success => {}
        }
    }

    // Handle the result
    match process_result {
        ProcessResult::Success => {
            // Acknowledge successful processing
            if let Err(e) = consumer.acknowledge_event(entry_id).await {
                error!(
                    entry_id = %entry_id,
                    error = %e,
                    "Failed to acknowledge event"
                );
            } else {
                info!(
                    entry_id = %entry_id,
                    instance_id = %trigger_event.instance_id,
                    duration_ms = (duration * 1000.0) as u64,
                    "Event processed and acknowledged"
                );
            }
        }
        ProcessResult::PermanentFailure(ref error_msg) => {
            // ACK to prevent infinite retry loops
            error!(
                entry_id = %entry_id,
                instance_id = %trigger_event.instance_id,
                error = %error_msg,
                duration_ms = (duration * 1000.0) as u64,
                "Event processing failed permanently"
            );
            if let Err(ack_err) = consumer.acknowledge_event(entry_id).await {
                error!(entry_id = %entry_id, error = %ack_err, "Failed to ACK failed event");
            }
        }
        ProcessResult::RetryLater(ref reason) => {
            // Check delivery count to enforce max retries
            let delivery_count = consumer.get_delivery_count(entry_id).await.unwrap_or(1); // Default to 1 if query fails

            if delivery_count >= max_retries {
                // Exceeded max retries - give up and ACK to prevent infinite loop
                error!(
                    entry_id = %entry_id,
                    instance_id = %trigger_event.instance_id,
                    reason = %reason,
                    delivery_count = delivery_count,
                    max_retries = max_retries,
                    "Event exceeded max retries, giving up"
                );
                // Record as failed
                if let Some(m) = metrics() {
                    m.trigger_events_failed.add(1, &attributes);
                }
                // ACK to remove from PEL
                if let Err(ack_err) = consumer.acknowledge_event(entry_id).await {
                    error!(entry_id = %entry_id, error = %ack_err, "Failed to ACK exhausted event");
                }
            } else if is_retry {
                warn!(
                    entry_id = %entry_id,
                    instance_id = %trigger_event.instance_id,
                    reason = %reason,
                    delivery_count = delivery_count,
                    max_retries = max_retries,
                    "Event will be retried later ({}/{} attempts)",
                    delivery_count, max_retries
                );
                // DON'T ACK - event stays in PEL for retry
            } else {
                info!(
                    entry_id = %entry_id,
                    instance_id = %trigger_event.instance_id,
                    reason = %reason,
                    delivery_count = delivery_count,
                    "Event will be retried later (not compiled)"
                );
                // DON'T ACK - event stays in PEL for retry
            }
        }
    }
}

/// Parse a TriggerEvent from a ValkeyEvent
fn parse_trigger_event(
    valkey_event: &crate::valkey::events::ValkeyEvent,
) -> Result<TriggerEvent, String> {
    // The TriggerEvent is stored as JSON in the "data" field
    let data = valkey_event
        .raw_data
        .get("data")
        .ok_or_else(|| "Missing 'data' field in stream event".to_string())?;

    serde_json::from_str(data).map_err(|e| format!("Failed to parse TriggerEvent JSON: {}", e))
}

/// Process a single trigger event
///
/// Launches the scenario via runtara-environment in detached mode.
/// All execution state is stored in runtara-environment, not in the local database.
/// Use the /api/runtime/executions endpoint to query execution status (proxies to runtara).
///
/// Returns:
/// - `ProcessResult::Success` if the instance was started successfully (or skipped due to single_instance)
/// - `ProcessResult::RetryLater` if the scenario is not compiled (will be retried)
/// - `ProcessResult::PermanentFailure` for other errors that won't benefit from retry
async fn process_trigger_event(
    engine: Arc<ExecutionEngine>,
    event: &TriggerEvent,
) -> ProcessResult {
    // Check single_instance constraint if this event has a trigger_id
    if let Some(trigger_id) = event.trigger_id() {
        match engine.get_trigger_single_instance(trigger_id).await {
            Ok(Some(true)) => {
                // single_instance is enabled - check for running instances
                match engine
                    .has_running_instance(&event.tenant_id, &event.scenario_id)
                    .await
                {
                    Ok(true) => {
                        // Instance already running - skip silently
                        info!(
                            instance_id = %event.instance_id,
                            scenario_id = %event.scenario_id,
                            trigger_id = %trigger_id,
                            trigger_type = %event.trigger_type(),
                            "Skipping execution: single_instance enabled and instance already running"
                        );
                        return ProcessResult::Success;
                    }
                    Ok(false) => {
                        // No running instance - proceed with execution
                    }
                    Err(e) => {
                        // Failed to check - log warning but proceed (fail-open)
                        warn!(
                            instance_id = %event.instance_id,
                            scenario_id = %event.scenario_id,
                            error = %e,
                            "Failed to check running instances, proceeding with execution"
                        );
                    }
                }
            }
            Ok(Some(false)) | Ok(None) => {
                // single_instance not enabled or trigger not found - proceed
            }
            Err(e) => {
                // Failed to get trigger - log warning but proceed (fail-open)
                warn!(
                    instance_id = %event.instance_id,
                    trigger_id = %trigger_id,
                    error = %e,
                    "Failed to get trigger config, proceeding with execution"
                );
            }
        }
    }

    // Launch instance via runtara-environment (fire-and-forget)
    // The runtara-environment server will:
    // - Execute the workflow in a container
    // - Track status (running, completed, failed, cancelled)
    // - Collect results
    // All execution data is queried directly from runtara-environment via the Management SDK.
    match engine.execute_detached(event).await {
        Ok(started_instance_id) => {
            info!(
                instance_id = %event.instance_id,
                runtara_instance_id = %started_instance_id,
                "Instance started via runtara-environment"
            );
            ProcessResult::Success
        }
        Err(ExecutionError::NotCompiled {
            scenario_id,
            version,
            compilation_queued,
        }) => {
            // Scenario not compiled yet - this is retryable
            // Compilation has been queued (or was already pending)
            info!(
                instance_id = %event.instance_id,
                scenario_id = %scenario_id,
                version = version,
                compilation_queued = compilation_queued,
                "Scenario not compiled, will retry after compilation"
            );
            ProcessResult::RetryLater(format!(
                "Scenario '{}' v{} not compiled (queued: {})",
                scenario_id, version, compilation_queued
            ))
        }
        Err(e) => {
            // Other errors are permanent failures
            error!(
                instance_id = %event.instance_id,
                error = %e,
                "Failed to start instance via runtara-environment"
            );
            ProcessResult::PermanentFailure(format!("Failed to start instance: {}", e))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_trigger_worker_config_struct() {
        // Test explicit configuration (don't rely on env vars due to Rust 2024 safety)
        let config = TriggerWorkerConfig {
            tenant_id: "test-tenant".to_string(),
            batch_size: 20,
            block_timeout_ms: 3000,
            pending_retry_delay_ms: 5000,
            pending_batch_size: 10,
            max_retries: 50,
        };

        assert_eq!(config.tenant_id, "test-tenant");
        assert_eq!(config.batch_size, 20);
        assert_eq!(config.block_timeout_ms, 3000);
        assert_eq!(config.pending_retry_delay_ms, 5000);
        assert_eq!(config.pending_batch_size, 10);
        assert_eq!(config.max_retries, 50);
    }

    #[test]
    fn test_trigger_worker_config_clone() {
        let config = TriggerWorkerConfig {
            tenant_id: "clone-test".to_string(),
            batch_size: 5,
            block_timeout_ms: 1000,
            pending_retry_delay_ms: 2000,
            pending_batch_size: 3,
            max_retries: 10,
        };

        let cloned = config.clone();
        assert_eq!(cloned.tenant_id, "clone-test");
        assert_eq!(cloned.pending_retry_delay_ms, 2000);
    }

    #[test]
    fn test_process_result_debug() {
        // Test that ProcessResult implements Debug correctly
        let success = ProcessResult::Success;
        let failure = ProcessResult::PermanentFailure("test error".to_string());
        let retry = ProcessResult::RetryLater("not compiled".to_string());

        assert!(format!("{:?}", success).contains("Success"));
        assert!(format!("{:?}", failure).contains("PermanentFailure"));
        assert!(format!("{:?}", failure).contains("test error"));
        assert!(format!("{:?}", retry).contains("RetryLater"));
        assert!(format!("{:?}", retry).contains("not compiled"));
    }

    #[test]
    fn test_parse_trigger_event_success() {
        let mut raw_data = HashMap::new();
        // Full TriggerEvent structure with all required fields
        raw_data.insert(
            "data".to_string(),
            r#"{
                "instance_id": "inst-123",
                "tenant_id": "tenant-1",
                "scenario_id": "scenario-abc",
                "version": 1,
                "inputs": {},
                "trigger": {"type": "http_api", "correlation_id": null},
                "requested_at": 1234567890000,
                "track_events": false
            }"#
            .to_string(),
        );

        let valkey_event = crate::valkey::events::ValkeyEvent {
            event_id: Some("1234-0".to_string()),
            event_type: Some("trigger_scenario".to_string()),
            scenario_id: Some("scenario-abc".to_string()),
            inputs: None,
            metadata: None,
            raw_data,
        };

        let result = parse_trigger_event(&valkey_event);
        assert!(result.is_ok(), "Parse failed: {:?}", result);

        let trigger = result.unwrap();
        assert_eq!(trigger.instance_id, "inst-123");
        assert_eq!(trigger.tenant_id, "tenant-1");
        assert_eq!(trigger.scenario_id, "scenario-abc");
        assert_eq!(trigger.version, Some(1));
        assert_eq!(trigger.trigger_type(), "http_api");
    }

    #[test]
    fn test_parse_trigger_event_no_version() {
        let mut raw_data = HashMap::new();
        // TriggerEvent with version = null (None)
        raw_data.insert(
            "data".to_string(),
            r#"{
                "instance_id": "inst-456",
                "tenant_id": "tenant-2",
                "scenario_id": "scenario-xyz",
                "version": null,
                "inputs": {"key": "value"},
                "trigger": {"type": "cron", "trigger_id": "cron-1", "schedule": "0 * * * *", "scheduled_at": 1234567890000},
                "requested_at": 1234567890000,
                "track_events": true
            }"#.to_string(),
        );

        let valkey_event = crate::valkey::events::ValkeyEvent {
            event_id: Some("1234-1".to_string()),
            event_type: Some("trigger_scenario".to_string()),
            scenario_id: Some("scenario-xyz".to_string()),
            inputs: None,
            metadata: None,
            raw_data,
        };

        let result = parse_trigger_event(&valkey_event);
        assert!(result.is_ok(), "Parse failed: {:?}", result);

        let trigger = result.unwrap();
        assert_eq!(trigger.instance_id, "inst-456");
        assert_eq!(trigger.version, None); // No version specified
        assert_eq!(trigger.trigger_type(), "cron");
        assert!(trigger.track_events);
    }

    #[test]
    fn test_parse_trigger_event_missing_data() {
        let valkey_event = crate::valkey::events::ValkeyEvent {
            event_id: Some("1234-0".to_string()),
            event_type: Some("trigger_scenario".to_string()),
            scenario_id: Some("scenario-abc".to_string()),
            inputs: None,
            metadata: None,
            raw_data: HashMap::new(), // No "data" field
        };

        let result = parse_trigger_event(&valkey_event);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Missing 'data' field"));
    }

    #[test]
    fn test_parse_trigger_event_invalid_json() {
        let mut raw_data = HashMap::new();
        raw_data.insert("data".to_string(), "not valid json".to_string());

        let valkey_event = crate::valkey::events::ValkeyEvent {
            event_id: Some("1234-0".to_string()),
            event_type: None,
            scenario_id: None,
            inputs: None,
            metadata: None,
            raw_data,
        };

        let result = parse_trigger_event(&valkey_event);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .contains("Failed to parse TriggerEvent JSON")
        );
    }
}
