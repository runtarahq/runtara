// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Instance protocol handlers for runtara-core.
//!
//! These handlers process requests from instances (registration, checkpoints, events, signals, etc.)

use std::{sync::Arc, time::Duration};

use anyhow::Result;
use chrono::{DateTime, Utc};
use tracing::{debug, info, instrument, warn};

use runtara_protocol::instance_proto as proto;
use runtara_protocol::instance_proto::{
    CheckpointRequest, CheckpointResponse, GetCheckpointRequest, GetCheckpointResponse,
    GetInstanceStatusRequest, GetInstanceStatusResponse, InstanceEvent, InstanceEventResponse,
    InstanceEventType, InstanceStatus, PollSignalsRequest, PollSignalsResponse,
    RegisterInstanceRequest, RegisterInstanceResponse, RetryAttemptEvent, Signal, SignalAck,
    SignalType, SleepRequest, SleepResponse,
};

use crate::error::CoreError;
use crate::persistence::{EventRecord, Persistence};

/// Shared state for instance handlers.
///
/// Contains the persistence implementation shared across all handlers.
pub struct InstanceHandlerState {
    /// Persistence implementation.
    pub persistence: Arc<dyn Persistence>,
}

impl InstanceHandlerState {
    /// Create a new instance handler state with the given persistence backend.
    pub fn new(persistence: Arc<dyn Persistence>) -> Self {
        Self { persistence }
    }
}

// ============================================================================
// Instance Registration
// ============================================================================

/// Handle instance registration request.
///
/// Registers an instance with Core, optionally resuming from a checkpoint.
/// If the instance doesn't exist, it's created (self-registration).
///
/// # Errors
///
/// Returns an error response if:
/// - `instance_id` or `tenant_id` is empty
/// - A specified `checkpoint_id` doesn't exist
#[instrument(skip(state, request), fields(instance_id = %request.instance_id))]
pub async fn handle_register_instance(
    state: &InstanceHandlerState,
    request: RegisterInstanceRequest,
) -> Result<RegisterInstanceResponse> {
    info!(
        tenant_id = %request.tenant_id,
        resuming_from = ?request.checkpoint_id,
        "Instance registering"
    );

    // 1. Validate instance_id is not empty
    if request.instance_id.is_empty() {
        return Ok(RegisterInstanceResponse {
            success: false,
            error: "instance_id is required".to_string(),
        });
    }

    // 2. Validate tenant_id is not empty
    if request.tenant_id.is_empty() {
        return Ok(RegisterInstanceResponse {
            success: false,
            error: "tenant_id is required".to_string(),
        });
    }

    // 3. If checkpoint_id provided, verify it exists
    if let Some(ref cp_id) = request.checkpoint_id {
        let checkpoint = state
            .persistence
            .load_checkpoint(&request.instance_id, cp_id)
            .await;
        match checkpoint {
            Ok(Some(_)) => {
                debug!(checkpoint_id = %cp_id, "Checkpoint found for resume");
            }
            Ok(None) => {
                return Ok(RegisterInstanceResponse {
                    success: false,
                    error: format!("Checkpoint '{}' not found", cp_id),
                });
            }
            Err(e) => {
                return Ok(RegisterInstanceResponse {
                    success: false,
                    error: format!("Failed to verify checkpoint: {}", e),
                });
            }
        }
    }

    // 4. Check if instance exists, create if not (self-registration)
    let instance_exists = state
        .persistence
        .get_instance(&request.instance_id)
        .await
        .map(|opt| opt.is_some())
        .unwrap_or(false);

    if !instance_exists {
        // Self-registration: create instance record
        info!("Instance not found, creating self-registered instance");
        if let Err(e) = state
            .persistence
            .register_instance(&request.instance_id, &request.tenant_id)
            .await
        {
            return Ok(RegisterInstanceResponse {
                success: false,
                error: format!("Failed to create instance: {}", e),
            });
        }
    }

    // 5. Update instance status to RUNNING
    let started_at = Utc::now();
    if let Err(e) = state
        .persistence
        .update_instance_status(&request.instance_id, "running", Some(started_at))
        .await
    {
        return Ok(RegisterInstanceResponse {
            success: false,
            error: format!("Failed to update instance status: {}", e),
        });
    }

    // 6. Insert started event
    let event = EventRecord {
        id: None,
        instance_id: request.instance_id.clone(),
        event_type: "started".to_string(),
        checkpoint_id: request.checkpoint_id.clone(),
        payload: None,
        created_at: started_at,
        subtype: None,
    };
    if let Err(e) = state.persistence.insert_event(&event).await {
        warn!("Failed to insert started event: {}", e);
        // Don't fail registration just because event logging failed
    }

    info!("Instance registered successfully");

    Ok(RegisterInstanceResponse {
        success: true,
        error: String::new(),
    })
}

// ============================================================================
// Checkpointing (append-only log)
// ============================================================================

/// Checkpoint handler - combines save and load semantics.
///
/// - If checkpoint with this ID exists, returns the existing state (for resume)
/// - If checkpoint doesn't exist, saves the state and returns empty (fresh execution)
///
/// Also serves as heartbeat - updates instance's last activity timestamp.
/// Includes pending signal information so instance can react to cancel/pause.
#[instrument(skip(state, request), fields(instance_id = %request.instance_id, checkpoint_id = %request.checkpoint_id))]
pub async fn handle_checkpoint(
    state: &InstanceHandlerState,
    request: CheckpointRequest,
) -> Result<CheckpointResponse> {
    debug!(
        state_size = request.state.len(),
        "Processing checkpoint request"
    );

    // 1. Validate instance exists and is running
    let instance = state.persistence.get_instance(&request.instance_id).await?;
    match instance {
        Some(inst) => {
            if inst.status != "running" {
                return Err(CoreError::InvalidInstanceState {
                    instance_id: request.instance_id.clone(),
                    expected: "running".to_string(),
                    actual: inst.status,
                }
                .into());
            }
        }
        None => {
            return Err(CoreError::InstanceNotFound {
                instance_id: request.instance_id.clone(),
            }
            .into());
        }
    }

    // 2. Check if checkpoint already exists
    if let Some(existing) = state
        .persistence
        .load_checkpoint(&request.instance_id, &request.checkpoint_id)
        .await?
    {
        debug!(
            checkpoint_id = %request.checkpoint_id,
            state_size = existing.state.len(),
            "Found existing checkpoint - returning for resume"
        );

        // Check for pending signal even when returning existing checkpoint
        let pending_signal =
            get_pending_signal(state.persistence.as_ref(), &request.instance_id).await;
        let custom_signal = state
            .persistence
            .take_pending_custom_signal(&request.instance_id, &request.checkpoint_id)
            .await?
            .map(|sig| Signal {
                instance_id: request.instance_id.clone(),
                signal_type: SignalType::SignalResume.into(), // Custom signals are scoped; type is unused
                payload: sig.payload.unwrap_or_default(),
            });

        return Ok(CheckpointResponse {
            found: true,
            state: existing.state,
            pending_signal,
            custom_signal: custom_signal.map(|sig| proto::CustomSignal {
                checkpoint_id: request.checkpoint_id.clone(),
                payload: sig.payload,
            }),
            last_error: None, // TODO: Fetch last error from error_history when available
        });
    }

    // 3. Checkpoint doesn't exist - save new checkpoint
    state
        .persistence
        .save_checkpoint(&request.instance_id, &request.checkpoint_id, &request.state)
        .await?;

    // 4. Update instance's current checkpoint_id
    state
        .persistence
        .update_instance_checkpoint(&request.instance_id, &request.checkpoint_id)
        .await?;

    // 5. Check for pending signals to include in response
    let pending_signal = get_pending_signal(state.persistence.as_ref(), &request.instance_id).await;
    let custom_signal = state
        .persistence
        .take_pending_custom_signal(&request.instance_id, &request.checkpoint_id)
        .await?
        .map(|sig| proto::CustomSignal {
            checkpoint_id: request.checkpoint_id.clone(),
            payload: sig.payload.unwrap_or_default(),
        });

    if pending_signal.is_some() || custom_signal.is_some() {
        debug!(
            ?pending_signal,
            has_custom = custom_signal.is_some(),
            "Checkpoint saved with pending signal"
        );
    } else {
        debug!("New checkpoint saved successfully");
    }

    Ok(CheckpointResponse {
        found: false,
        state: Vec::new(),
        pending_signal,
        custom_signal,
        last_error: None,
    })
}

/// Helper to get the pending instance-wide signal for an instance.
async fn get_pending_signal(persistence: &dyn Persistence, instance_id: &str) -> Option<Signal> {
    match persistence.get_pending_signal(instance_id).await {
        Ok(Some(signal)) => {
            let signal_type = match signal.signal_type.as_str() {
                "cancel" => SignalType::SignalCancel,
                "pause" => SignalType::SignalPause,
                "resume" => SignalType::SignalResume,
                _ => return None,
            };
            Some(Signal {
                instance_id: instance_id.to_string(),
                signal_type: signal_type.into(),
                payload: signal.payload.unwrap_or_default(),
            })
        }
        _ => None,
    }
}

/// Get checkpoint handler - read-only lookup without saving.
///
/// Returns the checkpoint state if found, or empty if not found.
#[instrument(skip(state, request), fields(instance_id = %request.instance_id, checkpoint_id = %request.checkpoint_id))]
pub async fn handle_get_checkpoint(
    state: &InstanceHandlerState,
    request: GetCheckpointRequest,
) -> Result<GetCheckpointResponse> {
    debug!("Looking up checkpoint (read-only)");

    // 1. Validate instance exists
    let instance = state.persistence.get_instance(&request.instance_id).await?;
    if instance.is_none() {
        return Err(CoreError::InstanceNotFound {
            instance_id: request.instance_id.clone(),
        }
        .into());
    }

    // 2. Look up checkpoint
    if let Some(checkpoint) = state
        .persistence
        .load_checkpoint(&request.instance_id, &request.checkpoint_id)
        .await?
    {
        debug!(
            checkpoint_id = %request.checkpoint_id,
            state_size = checkpoint.state.len(),
            "Checkpoint found"
        );
        return Ok(GetCheckpointResponse {
            found: true,
            state: checkpoint.state,
        });
    }

    debug!(checkpoint_id = %request.checkpoint_id, "Checkpoint not found");
    Ok(GetCheckpointResponse {
        found: false,
        state: Vec::new(),
    })
}

// ============================================================================
// Sleep/Wake
// ============================================================================

/// Handle durable sleep request.
///
/// Saves the checkpoint state before sleeping, then sleeps in-process.
/// This ensures the state is durable and can be restored if the process
/// is killed during the sleep.
#[instrument(skip(state, request), fields(instance_id = %request.instance_id, checkpoint_id = %request.checkpoint_id))]
pub async fn handle_sleep(
    state: &InstanceHandlerState,
    request: SleepRequest,
) -> Result<SleepResponse> {
    debug!(
        duration_ms = request.duration_ms,
        state_size = request.state.len(),
        "Processing sleep request"
    );

    // 1. Save checkpoint before sleeping (for durability)
    if !request.checkpoint_id.is_empty() {
        state
            .persistence
            .save_checkpoint(&request.instance_id, &request.checkpoint_id, &request.state)
            .await?;

        // Update instance's current checkpoint_id
        state
            .persistence
            .update_instance_checkpoint(&request.instance_id, &request.checkpoint_id)
            .await?;

        debug!(checkpoint_id = %request.checkpoint_id, "Sleep checkpoint saved");
    }

    // 2. Sleep in-process; environment may hibernate managed instances separately.
    tokio::time::sleep(Duration::from_millis(request.duration_ms)).await;
    Ok(SleepResponse {})
}

// ============================================================================
// Instance Events
// ============================================================================

/// Handle instance event.
///
/// Processes events from instances:
/// - **Heartbeat**: Update activity timestamp
/// - **Completed**: Mark instance as completed, store output
/// - **Failed**: Mark instance as failed, store error
/// - **Suspended**: Mark instance as suspended
/// - **Custom**: Store custom event for telemetry (debug events, etc.)
///
/// All events return `InstanceEventResponse` to acknowledge persistence.
/// This ensures no events are lost due to race conditions when the process exits.
#[instrument(skip(state, event), fields(instance_id = %event.instance_id))]
pub async fn handle_instance_event(
    state: &InstanceHandlerState,
    event: InstanceEvent,
) -> Result<InstanceEventResponse> {
    debug!(
        event_type = ?event.event_type,
        checkpoint_id = ?event.checkpoint_id,
        payload_size = event.payload.len(),
        timestamp_ms = event.timestamp_ms,
        "Received instance event"
    );

    // 1. Map proto event type to DB enum
    let event_type = map_event_type(event.event_type());

    // 2. Validate instance_id is not empty
    if event.instance_id.is_empty() {
        return Err(CoreError::ValidationError {
            field: "instance_id".to_string(),
            message: "instance_id is required".to_string(),
        }
        .into());
    }

    // 3. Determine timestamp
    let created_at = DateTime::from_timestamp_millis(event.timestamp_ms).unwrap_or_else(Utc::now);

    // 4. Insert event record
    let event_record = EventRecord {
        id: None,
        instance_id: event.instance_id.clone(),
        event_type: event_type.to_string(),
        checkpoint_id: event.checkpoint_id.clone(),
        payload: if event.payload.is_empty() {
            None
        } else {
            Some(event.payload.clone())
        },
        created_at,
        subtype: event.subtype.clone(),
    };
    state.persistence.insert_event(&event_record).await?;

    // 5. Update instance status based on event type
    // All events return a response to acknowledge persistence
    match event.event_type() {
        InstanceEventType::EventHeartbeat => {
            // Heartbeat is just an "I'm alive" signal - no state changes needed
            // The event was already logged above
            debug!("Heartbeat received");
        }
        InstanceEventType::EventCompleted => {
            let output = if event.payload.is_empty() {
                None
            } else {
                Some(event.payload.as_slice())
            };
            state
                .persistence
                .complete_instance(&event.instance_id, output, None)
                .await?;
            info!("Instance completed successfully");
        }
        InstanceEventType::EventFailed => {
            let error = if event.payload.is_empty() {
                "Unknown error"
            } else {
                std::str::from_utf8(&event.payload).unwrap_or("Unknown error (binary payload)")
            };
            state
                .persistence
                .complete_instance(&event.instance_id, None, Some(error))
                .await?;
            warn!(error = %error, "Instance failed");
        }
        InstanceEventType::EventSuspended => {
            state
                .persistence
                .update_instance_status(&event.instance_id, "suspended", None)
                .await?;
            info!("Instance suspended");
        }
        InstanceEventType::EventCustom => {
            // Custom events are just stored for telemetry - no state changes needed
            // The event was already logged above with its subtype
            debug!(subtype = ?event.subtype, "Custom event received");
        }
    }

    Ok(InstanceEventResponse {
        success: true,
        error: None,
    })
}

// ============================================================================
// Instance Status
// ============================================================================

/// Handle instance status query.
///
/// Returns the current status of an instance including:
/// - Current status (pending, running, suspended, completed, failed, cancelled)
/// - Last checkpoint ID
/// - Start/finish timestamps
/// - Output data (if completed) or error message (if failed)
#[instrument(skip(state, request), fields(instance_id = %request.instance_id))]
pub async fn handle_get_instance_status(
    state: &InstanceHandlerState,
    request: GetInstanceStatusRequest,
) -> Result<GetInstanceStatusResponse> {
    debug!("Getting instance status");

    let instance = state.persistence.get_instance(&request.instance_id).await?;

    match instance {
        Some(inst) => {
            let status = map_status(&inst.status);

            Ok(GetInstanceStatusResponse {
                instance_id: request.instance_id,
                status: status.into(),
                checkpoint_id: inst.checkpoint_id,
                started_at_ms: inst.started_at.map(|t| t.timestamp_millis()).unwrap_or(0),
                finished_at_ms: inst.finished_at.map(|t| t.timestamp_millis()),
                output: inst.output,
                error: inst.error,
            })
        }
        None => Ok(GetInstanceStatusResponse {
            instance_id: request.instance_id,
            status: InstanceStatus::StatusUnknown.into(),
            checkpoint_id: None,
            started_at_ms: 0,
            finished_at_ms: None,
            output: None,
            error: Some("Instance not found".to_string()),
        }),
    }
}

// ============================================================================
// Signal Polling (instance → core)
// ============================================================================

/// Handle signal polling request.
///
/// Returns the oldest pending signal for the instance, if any.
/// Signals are: cancel, pause, resume.
///
/// Note: The checkpoint response also includes pending signals for efficiency.
/// This endpoint is for explicit polling when not checkpointing.
#[instrument(skip(state, request), fields(instance_id = %request.instance_id))]
pub async fn handle_poll_signals(
    state: &InstanceHandlerState,
    request: PollSignalsRequest,
) -> Result<PollSignalsResponse> {
    debug!("Instance polling for signals");

    let pending = state
        .persistence
        .get_pending_signal(&request.instance_id)
        .await?;
    let custom = if let Some(checkpoint_id) = request.checkpoint_id.as_deref() {
        state
            .persistence
            .take_pending_custom_signal(&request.instance_id, checkpoint_id)
            .await?
    } else {
        None
    };

    let signal = pending.map(|sig| {
        let signal_type = match sig.signal_type.as_str() {
            "cancel" => SignalType::SignalCancel,
            "pause" => SignalType::SignalPause,
            "resume" => SignalType::SignalResume,
            _ => {
                warn!(signal_type = %sig.signal_type, "Unknown signal type");
                SignalType::SignalCancel
            }
        };

        Signal {
            instance_id: request.instance_id.clone(),
            signal_type: signal_type.into(),
            payload: sig.payload.unwrap_or_default(),
        }
    });

    let custom_signal = custom.map(|sig| proto::CustomSignal {
        checkpoint_id: sig.checkpoint_id,
        payload: sig.payload.unwrap_or_default(),
    });

    if signal.is_some() || custom_signal.is_some() {
        debug!(
            has_global = signal.is_some(),
            has_custom = custom_signal.is_some(),
            "Returning pending signals"
        );
    }

    Ok(PollSignalsResponse {
        signal,
        custom_signal,
    })
}

/// Handle signal acknowledgement (fire-and-forget).
///
/// Marks a signal as acknowledged by the instance.
/// If acknowledging a cancel signal, also updates instance status to cancelled.
#[instrument(skip(state, ack), fields(instance_id = %ack.instance_id))]
pub async fn handle_signal_ack(state: &InstanceHandlerState, ack: SignalAck) -> Result<()> {
    debug!(
        signal_type = ?ack.signal_type,
        acknowledged = ack.acknowledged,
        "Received signal acknowledgement"
    );

    if ack.acknowledged {
        // Mark signal as acknowledged
        state
            .persistence
            .acknowledge_signal(&ack.instance_id)
            .await?;

        // Handle signal-specific side effects
        match ack.signal_type() {
            SignalType::SignalCancel => {
                // Update instance status to cancelled with finished_at
                state
                    .persistence
                    .complete_instance_extended(
                        &ack.instance_id,
                        "cancelled",
                        None, // output
                        None, // error
                        None, // stderr
                        None, // checkpoint_id
                    )
                    .await?;
                info!("Instance cancelled");
            }
            SignalType::SignalPause => {
                // Instance should checkpoint and suspend
                debug!("Pause signal acknowledged");
            }
            SignalType::SignalResume => {
                // Instance should resume execution
                debug!("Resume signal acknowledged");
            }
        }
    } else {
        warn!("Signal was not acknowledged by instance");
    }

    Ok(())
}

// ============================================================================
// Retry Tracking
// ============================================================================

/// Handle retry attempt event (fire-and-forget).
///
/// Records a retry attempt for audit trail. Retry attempts are stored
/// in the checkpoints table with `is_retry_attempt=true`.
///
/// This is sent by the SDK when a durable function fails and is about
/// to be retried (before the backoff delay).
#[instrument(skip(state, event), fields(instance_id = %event.instance_id, checkpoint_id = %event.checkpoint_id))]
pub async fn handle_retry_attempt(
    state: &InstanceHandlerState,
    event: RetryAttemptEvent,
) -> Result<()> {
    debug!(
        attempt = event.attempt_number,
        error = ?event.error_message,
        timestamp_ms = event.timestamp_ms,
        "Recording retry attempt"
    );

    // Save retry attempt record for audit trail
    state
        .persistence
        .save_retry_attempt(
            &event.instance_id,
            &event.checkpoint_id,
            event.attempt_number as i32,
            event.error_message.as_deref(),
        )
        .await?;

    debug!(attempt = event.attempt_number, "Retry attempt recorded");

    Ok(())
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Map proto event type to database enum string.
pub fn map_event_type(event_type: InstanceEventType) -> &'static str {
    match event_type {
        InstanceEventType::EventHeartbeat => "heartbeat",
        InstanceEventType::EventCompleted => "completed",
        InstanceEventType::EventFailed => "failed",
        InstanceEventType::EventSuspended => "suspended",
        InstanceEventType::EventCustom => "custom",
    }
}

/// Map database status string to proto enum.
pub fn map_status(status: &str) -> InstanceStatus {
    match status {
        "pending" => InstanceStatus::StatusPending,
        "running" => InstanceStatus::StatusRunning,
        "suspended" => InstanceStatus::StatusSuspended,
        "completed" => InstanceStatus::StatusCompleted,
        "failed" => InstanceStatus::StatusFailed,
        "cancelled" => InstanceStatus::StatusCancelled,
        _ => InstanceStatus::StatusUnknown,
    }
}

/// Map proto signal type to database enum string.
pub fn map_signal_type(signal_type: SignalType) -> &'static str {
    match signal_type {
        SignalType::SignalCancel => "cancel",
        SignalType::SignalPause => "pause",
        SignalType::SignalResume => "resume",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::{
        CheckpointRecord, CustomSignalRecord, InstanceRecord, ListEventsFilter,
        ListStepSummariesFilter, SignalRecord, StepSummaryRecord,
    };
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// Mock persistence for handler unit tests.
    struct MockPersistence {
        instances: Mutex<HashMap<String, InstanceRecord>>,
        checkpoints: Mutex<HashMap<(String, String), CheckpointRecord>>,
        signals: Mutex<HashMap<String, SignalRecord>>,
        events: Mutex<Vec<EventRecord>>,
        custom_signals: Mutex<HashMap<(String, String), CustomSignalRecord>>,
        fail_register: Mutex<bool>,
        fail_status_update: Mutex<bool>,
    }

    impl MockPersistence {
        fn new() -> Self {
            Self {
                instances: Mutex::new(HashMap::new()),
                checkpoints: Mutex::new(HashMap::new()),
                signals: Mutex::new(HashMap::new()),
                events: Mutex::new(Vec::new()),
                custom_signals: Mutex::new(HashMap::new()),
                fail_register: Mutex::new(false),
                fail_status_update: Mutex::new(false),
            }
        }

        fn with_instance(self, instance: InstanceRecord) -> Self {
            self.instances
                .lock()
                .unwrap()
                .insert(instance.instance_id.clone(), instance);
            self
        }

        fn with_checkpoint(self, checkpoint: CheckpointRecord) -> Self {
            self.checkpoints.lock().unwrap().insert(
                (
                    checkpoint.instance_id.clone(),
                    checkpoint.checkpoint_id.clone(),
                ),
                checkpoint,
            );
            self
        }

        fn with_signal(self, signal: SignalRecord) -> Self {
            self.signals
                .lock()
                .unwrap()
                .insert(signal.instance_id.clone(), signal);
            self
        }

        fn with_custom_signal(self, signal: CustomSignalRecord) -> Self {
            self.custom_signals.lock().unwrap().insert(
                (signal.instance_id.clone(), signal.checkpoint_id.clone()),
                signal,
            );
            self
        }

        #[allow(dead_code)]
        fn set_fail_register(&self) {
            *self.fail_register.lock().unwrap() = true;
        }

        #[allow(dead_code)]
        fn set_fail_status_update(&self) {
            *self.fail_status_update.lock().unwrap() = true;
        }

        fn get_events(&self) -> Vec<EventRecord> {
            self.events.lock().unwrap().clone()
        }
    }

    fn make_instance(instance_id: &str, tenant_id: &str, status: &str) -> InstanceRecord {
        InstanceRecord {
            instance_id: instance_id.to_string(),
            tenant_id: tenant_id.to_string(),
            definition_version: 1,
            status: status.to_string(),
            checkpoint_id: None,
            attempt: 1,
            max_attempts: 3,
            created_at: Utc::now(),
            started_at: None,
            finished_at: None,
            output: None,
            error: None,
            sleep_until: None,
        }
    }

    fn make_checkpoint(instance_id: &str, checkpoint_id: &str, state: &[u8]) -> CheckpointRecord {
        CheckpointRecord {
            id: 1,
            instance_id: instance_id.to_string(),
            checkpoint_id: checkpoint_id.to_string(),
            state: state.to_vec(),
            created_at: Utc::now(),
            is_compensatable: false,
            compensation_step_id: None,
            compensation_data: None,
            compensation_state: None,
            compensation_order: 0,
        }
    }

    fn make_signal(instance_id: &str, signal_type: &str) -> SignalRecord {
        SignalRecord {
            instance_id: instance_id.to_string(),
            signal_type: signal_type.to_string(),
            payload: None,
            created_at: Utc::now(),
            acknowledged_at: None,
        }
    }

    #[async_trait]
    impl Persistence for MockPersistence {
        async fn register_instance(
            &self,
            instance_id: &str,
            tenant_id: &str,
        ) -> std::result::Result<(), CoreError> {
            if *self.fail_register.lock().unwrap() {
                return Err(CoreError::DatabaseError {
                    operation: "register_instance".to_string(),
                    details: "Mock register failure".to_string(),
                });
            }
            let instance = make_instance(instance_id, tenant_id, "pending");
            self.instances
                .lock()
                .unwrap()
                .insert(instance_id.to_string(), instance);
            Ok(())
        }

        async fn get_instance(
            &self,
            instance_id: &str,
        ) -> std::result::Result<Option<InstanceRecord>, CoreError> {
            Ok(self.instances.lock().unwrap().get(instance_id).cloned())
        }

        async fn update_instance_status(
            &self,
            instance_id: &str,
            status: &str,
            started_at: Option<DateTime<Utc>>,
        ) -> std::result::Result<(), CoreError> {
            if *self.fail_status_update.lock().unwrap() {
                return Err(CoreError::DatabaseError {
                    operation: "update_instance_status".to_string(),
                    details: "Mock status update failure".to_string(),
                });
            }
            if let Some(inst) = self.instances.lock().unwrap().get_mut(instance_id) {
                inst.status = status.to_string();
                inst.started_at = started_at;
            }
            Ok(())
        }

        async fn update_instance_checkpoint(
            &self,
            instance_id: &str,
            checkpoint_id: &str,
        ) -> std::result::Result<(), CoreError> {
            if let Some(inst) = self.instances.lock().unwrap().get_mut(instance_id) {
                inst.checkpoint_id = Some(checkpoint_id.to_string());
            }
            Ok(())
        }

        async fn complete_instance(
            &self,
            instance_id: &str,
            output: Option<&[u8]>,
            error: Option<&str>,
        ) -> std::result::Result<(), CoreError> {
            if let Some(inst) = self.instances.lock().unwrap().get_mut(instance_id) {
                inst.status = if error.is_some() {
                    "failed".to_string()
                } else {
                    "completed".to_string()
                };
                inst.output = output.map(|o| o.to_vec());
                inst.error = error.map(|e| e.to_string());
                inst.finished_at = Some(Utc::now());
            }
            Ok(())
        }

        async fn save_checkpoint(
            &self,
            instance_id: &str,
            checkpoint_id: &str,
            state: &[u8],
        ) -> std::result::Result<(), CoreError> {
            let cp = make_checkpoint(instance_id, checkpoint_id, state);
            self.checkpoints
                .lock()
                .unwrap()
                .insert((instance_id.to_string(), checkpoint_id.to_string()), cp);
            Ok(())
        }

        async fn load_checkpoint(
            &self,
            instance_id: &str,
            checkpoint_id: &str,
        ) -> std::result::Result<Option<CheckpointRecord>, CoreError> {
            Ok(self
                .checkpoints
                .lock()
                .unwrap()
                .get(&(instance_id.to_string(), checkpoint_id.to_string()))
                .cloned())
        }

        async fn list_checkpoints(
            &self,
            _instance_id: &str,
            _checkpoint_id: Option<&str>,
            _limit: i64,
            _offset: i64,
            _created_after: Option<DateTime<Utc>>,
            _created_before: Option<DateTime<Utc>>,
        ) -> std::result::Result<Vec<CheckpointRecord>, CoreError> {
            Ok(Vec::new())
        }

        async fn count_checkpoints(
            &self,
            _instance_id: &str,
            _checkpoint_id: Option<&str>,
            _created_after: Option<DateTime<Utc>>,
            _created_before: Option<DateTime<Utc>>,
        ) -> std::result::Result<i64, CoreError> {
            Ok(0)
        }

        async fn insert_event(&self, event: &EventRecord) -> std::result::Result<(), CoreError> {
            self.events.lock().unwrap().push(event.clone());
            Ok(())
        }

        async fn insert_signal(
            &self,
            instance_id: &str,
            signal_type: &str,
            _payload: &[u8],
        ) -> std::result::Result<(), CoreError> {
            let signal = make_signal(instance_id, signal_type);
            self.signals
                .lock()
                .unwrap()
                .insert(instance_id.to_string(), signal);
            Ok(())
        }

        async fn get_pending_signal(
            &self,
            instance_id: &str,
        ) -> std::result::Result<Option<SignalRecord>, CoreError> {
            Ok(self.signals.lock().unwrap().get(instance_id).cloned())
        }

        async fn acknowledge_signal(
            &self,
            instance_id: &str,
        ) -> std::result::Result<(), CoreError> {
            self.signals.lock().unwrap().remove(instance_id);
            Ok(())
        }

        async fn insert_custom_signal(
            &self,
            _instance_id: &str,
            _checkpoint_id: &str,
            _payload: &[u8],
        ) -> std::result::Result<(), CoreError> {
            Ok(())
        }

        async fn take_pending_custom_signal(
            &self,
            instance_id: &str,
            checkpoint_id: &str,
        ) -> std::result::Result<Option<CustomSignalRecord>, CoreError> {
            Ok(self
                .custom_signals
                .lock()
                .unwrap()
                .remove(&(instance_id.to_string(), checkpoint_id.to_string())))
        }

        async fn save_retry_attempt(
            &self,
            _instance_id: &str,
            _checkpoint_id: &str,
            _attempt: i32,
            _error_message: Option<&str>,
        ) -> std::result::Result<(), CoreError> {
            Ok(())
        }

        async fn list_instances(
            &self,
            _tenant_id: Option<&str>,
            _status: Option<&str>,
            _limit: i64,
            _offset: i64,
        ) -> std::result::Result<Vec<InstanceRecord>, CoreError> {
            Ok(Vec::new())
        }

        async fn health_check_db(&self) -> std::result::Result<bool, CoreError> {
            Ok(true)
        }

        async fn count_active_instances(&self) -> std::result::Result<i64, CoreError> {
            Ok(0)
        }

        async fn set_instance_sleep(
            &self,
            _instance_id: &str,
            _sleep_until: DateTime<Utc>,
        ) -> std::result::Result<(), CoreError> {
            Ok(())
        }

        async fn clear_instance_sleep(
            &self,
            _instance_id: &str,
        ) -> std::result::Result<(), CoreError> {
            Ok(())
        }

        async fn get_sleeping_instances_due(
            &self,
            _limit: i64,
        ) -> std::result::Result<Vec<InstanceRecord>, CoreError> {
            Ok(Vec::new())
        }

        async fn list_events(
            &self,
            _instance_id: &str,
            _filter: &ListEventsFilter,
            _limit: i64,
            _offset: i64,
        ) -> std::result::Result<Vec<EventRecord>, CoreError> {
            Ok(Vec::new())
        }

        async fn count_events(
            &self,
            _instance_id: &str,
            _filter: &ListEventsFilter,
        ) -> std::result::Result<i64, CoreError> {
            Ok(0)
        }

        async fn list_step_summaries(
            &self,
            _instance_id: &str,
            _filter: &ListStepSummariesFilter,
            _limit: i64,
            _offset: i64,
        ) -> std::result::Result<Vec<StepSummaryRecord>, CoreError> {
            Ok(Vec::new())
        }

        async fn count_step_summaries(
            &self,
            _instance_id: &str,
            _filter: &ListStepSummariesFilter,
        ) -> std::result::Result<i64, CoreError> {
            Ok(0)
        }
    }

    #[test]
    fn test_event_type_mapping() {
        assert_eq!(
            map_event_type(InstanceEventType::EventHeartbeat),
            "heartbeat"
        );
        assert_eq!(
            map_event_type(InstanceEventType::EventCompleted),
            "completed"
        );
        assert_eq!(map_event_type(InstanceEventType::EventFailed), "failed");
        assert_eq!(
            map_event_type(InstanceEventType::EventSuspended),
            "suspended"
        );
        assert_eq!(map_event_type(InstanceEventType::EventCustom), "custom");
    }

    #[test]
    fn test_status_mapping_all_variants() {
        assert_eq!(map_status("pending"), InstanceStatus::StatusPending);
        assert_eq!(map_status("running"), InstanceStatus::StatusRunning);
        assert_eq!(map_status("suspended"), InstanceStatus::StatusSuspended);
        assert_eq!(map_status("completed"), InstanceStatus::StatusCompleted);
        assert_eq!(map_status("failed"), InstanceStatus::StatusFailed);
        assert_eq!(map_status("cancelled"), InstanceStatus::StatusCancelled);
        assert_eq!(map_status("invalid"), InstanceStatus::StatusUnknown);
        assert_eq!(map_status(""), InstanceStatus::StatusUnknown);
    }

    #[test]
    fn test_signal_type_mapping() {
        assert_eq!(map_signal_type(SignalType::SignalCancel), "cancel");
        assert_eq!(map_signal_type(SignalType::SignalPause), "pause");
        assert_eq!(map_signal_type(SignalType::SignalResume), "resume");
    }

    #[test]
    fn test_instance_handler_state_new() {
        let persistence = Arc::new(MockPersistence::new());
        let state = InstanceHandlerState::new(persistence);
        // Just verify it compiles and persistence is accessible
        let _ = &state.persistence;
    }

    // ========================================================================
    // Register Instance Handler Tests
    // ========================================================================

    #[tokio::test]
    async fn test_register_empty_instance_id() {
        let persistence = Arc::new(MockPersistence::new());
        let state = InstanceHandlerState::new(persistence);

        let request = RegisterInstanceRequest {
            instance_id: "".to_string(),
            tenant_id: "tenant-1".to_string(),
            checkpoint_id: None,
        };

        let result = handle_register_instance(&state, request).await.unwrap();
        assert!(!result.success);
        assert!(result.error.contains("instance_id is required"));
    }

    #[tokio::test]
    async fn test_register_empty_tenant_id() {
        let persistence = Arc::new(MockPersistence::new());
        let state = InstanceHandlerState::new(persistence);

        let request = RegisterInstanceRequest {
            instance_id: "inst-1".to_string(),
            tenant_id: "".to_string(),
            checkpoint_id: None,
        };

        let result = handle_register_instance(&state, request).await.unwrap();
        assert!(!result.success);
        assert!(result.error.contains("tenant_id is required"));
    }

    #[tokio::test]
    async fn test_register_self_registration() {
        let persistence = Arc::new(MockPersistence::new());
        let state = InstanceHandlerState::new(persistence);

        let request = RegisterInstanceRequest {
            instance_id: "inst-new".to_string(),
            tenant_id: "tenant-1".to_string(),
            checkpoint_id: None,
        };

        let result = handle_register_instance(&state, request).await.unwrap();
        assert!(result.success);
        assert!(result.error.is_empty());
    }

    #[tokio::test]
    async fn test_register_existing_instance() {
        let persistence = Arc::new(
            MockPersistence::new().with_instance(make_instance("inst-1", "tenant-1", "pending")),
        );
        let state = InstanceHandlerState::new(persistence);

        let request = RegisterInstanceRequest {
            instance_id: "inst-1".to_string(),
            tenant_id: "tenant-1".to_string(),
            checkpoint_id: None,
        };

        let result = handle_register_instance(&state, request).await.unwrap();
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_register_with_valid_checkpoint() {
        let persistence = Arc::new(
            MockPersistence::new()
                .with_instance(make_instance("inst-1", "tenant-1", "pending"))
                .with_checkpoint(make_checkpoint("inst-1", "cp-1", b"state")),
        );
        let state = InstanceHandlerState::new(persistence);

        let request = RegisterInstanceRequest {
            instance_id: "inst-1".to_string(),
            tenant_id: "tenant-1".to_string(),
            checkpoint_id: Some("cp-1".to_string()),
        };

        let result = handle_register_instance(&state, request).await.unwrap();
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_register_with_invalid_checkpoint() {
        let persistence = Arc::new(
            MockPersistence::new().with_instance(make_instance("inst-1", "tenant-1", "pending")),
        );
        let state = InstanceHandlerState::new(persistence);

        let request = RegisterInstanceRequest {
            instance_id: "inst-1".to_string(),
            tenant_id: "tenant-1".to_string(),
            checkpoint_id: Some("nonexistent".to_string()),
        };

        let result = handle_register_instance(&state, request).await.unwrap();
        assert!(!result.success);
        assert!(result.error.contains("not found"));
    }

    #[tokio::test]
    async fn test_register_creates_started_event() {
        let mock = MockPersistence::new();
        let persistence = Arc::new(mock);
        let state = InstanceHandlerState::new(persistence.clone());

        let request = RegisterInstanceRequest {
            instance_id: "inst-1".to_string(),
            tenant_id: "tenant-1".to_string(),
            checkpoint_id: None,
        };

        let result = handle_register_instance(&state, request).await.unwrap();
        assert!(result.success);

        // Check that started event was created
        let events = persistence.get_events();
        assert!(!events.is_empty());
        assert_eq!(events[0].event_type, "started");
        assert_eq!(events[0].instance_id, "inst-1");
    }

    // ========================================================================
    // Checkpoint Handler Tests
    // ========================================================================

    #[tokio::test]
    async fn test_checkpoint_instance_not_found() {
        let persistence = Arc::new(MockPersistence::new());
        let state = InstanceHandlerState::new(persistence);

        let request = CheckpointRequest {
            instance_id: "nonexistent".to_string(),
            checkpoint_id: "cp-1".to_string(),
            state: b"test state".to_vec(),
        };

        let result = handle_checkpoint(&state, request).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_checkpoint_instance_not_running() {
        let persistence = Arc::new(MockPersistence::new().with_instance(make_instance(
            "inst-1",
            "tenant-1",
            "completed",
        )));
        let state = InstanceHandlerState::new(persistence);

        let request = CheckpointRequest {
            instance_id: "inst-1".to_string(),
            checkpoint_id: "cp-1".to_string(),
            state: b"test state".to_vec(),
        };

        let result = handle_checkpoint(&state, request).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_checkpoint_new_saves_state() {
        let persistence = Arc::new(
            MockPersistence::new().with_instance(make_instance("inst-1", "tenant-1", "running")),
        );
        let state = InstanceHandlerState::new(persistence);

        let request = CheckpointRequest {
            instance_id: "inst-1".to_string(),
            checkpoint_id: "cp-1".to_string(),
            state: b"test state".to_vec(),
        };

        let result = handle_checkpoint(&state, request).await.unwrap();
        assert!(!result.found); // New checkpoint, not found
    }

    #[tokio::test]
    async fn test_checkpoint_existing_returns_state() {
        let persistence = Arc::new(
            MockPersistence::new()
                .with_instance(make_instance("inst-1", "tenant-1", "running"))
                .with_checkpoint(make_checkpoint("inst-1", "cp-1", b"existing state")),
        );
        let state = InstanceHandlerState::new(persistence);

        let request = CheckpointRequest {
            instance_id: "inst-1".to_string(),
            checkpoint_id: "cp-1".to_string(),
            state: b"new state".to_vec(), // This should be ignored
        };

        let result = handle_checkpoint(&state, request).await.unwrap();
        assert!(result.found);
        assert_eq!(result.state, b"existing state");
    }

    #[tokio::test]
    async fn test_checkpoint_returns_pending_signal() {
        let persistence = Arc::new(
            MockPersistence::new()
                .with_instance(make_instance("inst-1", "tenant-1", "running"))
                .with_signal(make_signal("inst-1", "cancel")),
        );
        let state = InstanceHandlerState::new(persistence);

        let request = CheckpointRequest {
            instance_id: "inst-1".to_string(),
            checkpoint_id: "cp-1".to_string(),
            state: b"test state".to_vec(),
        };

        let result = handle_checkpoint(&state, request).await.unwrap();
        assert!(result.pending_signal.is_some());
        let signal = result.pending_signal.unwrap();
        assert_eq!(signal.signal_type, SignalType::SignalCancel as i32);
    }

    #[tokio::test]
    async fn test_checkpoint_returns_custom_signal() {
        let custom_signal = CustomSignalRecord {
            instance_id: "inst-1".to_string(),
            checkpoint_id: "cp-1".to_string(),
            payload: Some(b"custom payload".to_vec()),
            created_at: Utc::now(),
        };
        let persistence = Arc::new(
            MockPersistence::new()
                .with_instance(make_instance("inst-1", "tenant-1", "running"))
                .with_custom_signal(custom_signal),
        );
        let state = InstanceHandlerState::new(persistence);

        let request = CheckpointRequest {
            instance_id: "inst-1".to_string(),
            checkpoint_id: "cp-1".to_string(),
            state: b"test state".to_vec(),
        };

        let result = handle_checkpoint(&state, request).await.unwrap();
        assert!(result.custom_signal.is_some());
        let cs = result.custom_signal.unwrap();
        assert_eq!(cs.checkpoint_id, "cp-1");
        assert_eq!(cs.payload, b"custom payload");
    }

    // ========================================================================
    // Get Instance Status Handler Tests
    // ========================================================================

    #[tokio::test]
    async fn test_get_status_not_found() {
        let persistence = Arc::new(MockPersistence::new());
        let state = InstanceHandlerState::new(persistence);

        let request = GetInstanceStatusRequest {
            instance_id: "nonexistent".to_string(),
        };

        let result = handle_get_instance_status(&state, request).await.unwrap();
        // Instance not found returns StatusUnknown
        assert_eq!(result.status, InstanceStatus::StatusUnknown as i32);
    }

    #[tokio::test]
    async fn test_get_status_found() {
        let persistence = Arc::new(
            MockPersistence::new().with_instance(make_instance("inst-1", "tenant-1", "running")),
        );
        let state = InstanceHandlerState::new(persistence);

        let request = GetInstanceStatusRequest {
            instance_id: "inst-1".to_string(),
        };

        let result = handle_get_instance_status(&state, request).await.unwrap();
        assert_eq!(result.status, InstanceStatus::StatusRunning as i32);
    }

    // ========================================================================
    // Poll Signals Handler Tests
    // ========================================================================

    #[tokio::test]
    async fn test_poll_signals_no_signal() {
        let persistence = Arc::new(
            MockPersistence::new().with_instance(make_instance("inst-1", "tenant-1", "running")),
        );
        let state = InstanceHandlerState::new(persistence);

        let request = PollSignalsRequest {
            instance_id: "inst-1".to_string(),
            checkpoint_id: None,
        };

        let result = handle_poll_signals(&state, request).await.unwrap();
        assert!(result.signal.is_none());
    }

    #[tokio::test]
    async fn test_poll_signals_with_pending_signal() {
        let persistence = Arc::new(
            MockPersistence::new()
                .with_instance(make_instance("inst-1", "tenant-1", "running"))
                .with_signal(make_signal("inst-1", "pause")),
        );
        let state = InstanceHandlerState::new(persistence);

        let request = PollSignalsRequest {
            instance_id: "inst-1".to_string(),
            checkpoint_id: None,
        };

        let result = handle_poll_signals(&state, request).await.unwrap();
        assert!(result.signal.is_some());
        let signal = result.signal.unwrap();
        assert_eq!(signal.signal_type, SignalType::SignalPause as i32);
    }

    // ========================================================================
    // Signal Ack Handler Tests
    // ========================================================================

    #[tokio::test]
    async fn test_signal_ack_success() {
        let persistence = Arc::new(
            MockPersistence::new()
                .with_instance(make_instance("inst-1", "tenant-1", "running"))
                .with_signal(make_signal("inst-1", "cancel")),
        );
        let state = InstanceHandlerState::new(persistence.clone());

        let request = SignalAck {
            instance_id: "inst-1".to_string(),
            signal_type: SignalType::SignalCancel as i32,
            acknowledged: true,
        };

        // handle_signal_ack returns Result<()>
        handle_signal_ack(&state, request).await.unwrap();

        // Verify signal was acknowledged (removed from pending)
        assert!(
            persistence
                .get_pending_signal("inst-1")
                .await
                .unwrap()
                .is_none()
        );
    }

    // ========================================================================
    // Instance Event Handler Tests
    // ========================================================================

    #[tokio::test]
    async fn test_handle_event_heartbeat() {
        let persistence = Arc::new(
            MockPersistence::new().with_instance(make_instance("inst-1", "tenant-1", "running")),
        );
        let state = InstanceHandlerState::new(persistence.clone());

        let event = InstanceEvent {
            instance_id: "inst-1".to_string(),
            event_type: InstanceEventType::EventHeartbeat as i32,
            checkpoint_id: None,
            payload: Vec::new(),
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
            subtype: None,
        };

        let result = handle_instance_event(&state, event).await.unwrap();
        assert!(result.success);

        // Verify event was inserted
        let events = persistence.get_events();
        assert!(!events.is_empty());
        assert_eq!(events[0].event_type, "heartbeat");
    }

    #[tokio::test]
    async fn test_handle_event_completed() {
        let persistence = Arc::new(
            MockPersistence::new().with_instance(make_instance("inst-1", "tenant-1", "running")),
        );
        let state = InstanceHandlerState::new(persistence.clone());

        let event = InstanceEvent {
            instance_id: "inst-1".to_string(),
            event_type: InstanceEventType::EventCompleted as i32,
            checkpoint_id: None,
            payload: b"result".to_vec(),
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
            subtype: None,
        };

        let result = handle_instance_event(&state, event).await.unwrap();
        assert!(result.success);

        // Verify instance was completed
        let inst = persistence.get_instance("inst-1").await.unwrap().unwrap();
        assert_eq!(inst.status, "completed");
    }

    #[tokio::test]
    async fn test_handle_event_failed() {
        let persistence = Arc::new(
            MockPersistence::new().with_instance(make_instance("inst-1", "tenant-1", "running")),
        );
        let state = InstanceHandlerState::new(persistence.clone());

        let event = InstanceEvent {
            instance_id: "inst-1".to_string(),
            event_type: InstanceEventType::EventFailed as i32,
            checkpoint_id: None,
            payload: b"error message".to_vec(),
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
            subtype: None,
        };

        let result = handle_instance_event(&state, event).await.unwrap();
        assert!(result.success);

        // Verify instance was failed
        let inst = persistence.get_instance("inst-1").await.unwrap().unwrap();
        assert_eq!(inst.status, "failed");
    }

    #[tokio::test]
    async fn test_handle_event_suspended() {
        let persistence = Arc::new(
            MockPersistence::new().with_instance(make_instance("inst-1", "tenant-1", "running")),
        );
        let state = InstanceHandlerState::new(persistence.clone());

        let event = InstanceEvent {
            instance_id: "inst-1".to_string(),
            event_type: InstanceEventType::EventSuspended as i32,
            checkpoint_id: None,
            payload: Vec::new(),
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
            subtype: None,
        };

        let result = handle_instance_event(&state, event).await.unwrap();
        assert!(result.success);

        // Verify instance was suspended
        let inst = persistence.get_instance("inst-1").await.unwrap().unwrap();
        assert_eq!(inst.status, "suspended");
    }

    #[tokio::test]
    async fn test_handle_event_custom() {
        let persistence = Arc::new(
            MockPersistence::new().with_instance(make_instance("inst-1", "tenant-1", "running")),
        );
        let state = InstanceHandlerState::new(persistence.clone());

        let event = InstanceEvent {
            instance_id: "inst-1".to_string(),
            event_type: InstanceEventType::EventCustom as i32,
            checkpoint_id: None,
            payload: b"custom data".to_vec(),
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
            subtype: Some("my_custom_type".to_string()),
        };

        let result = handle_instance_event(&state, event).await.unwrap();
        assert!(result.success);

        // Verify event was inserted with subtype
        let events = persistence.get_events();
        assert!(!events.is_empty());
        assert_eq!(events[0].event_type, "custom");
        assert_eq!(events[0].subtype.as_deref(), Some("my_custom_type"));
    }
}
