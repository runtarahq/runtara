// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Embedded SDK backend for direct persistence access.
//!
//! This backend calls the persistence layer directly,
//! suitable for embedding runtara-core within the same process.

use runtara_core::domain::EventType as CoreEventType;
use runtara_core::domain::InstanceStatus as CoreInstanceStatus;
use runtara_core::domain::SignalType as CoreSignalType;

use std::sync::Arc;
use std::time::Duration;

use crate::tracing_compat::{debug, info};
use chrono::{DateTime, Utc};
use runtara_core::TenantId;
use runtara_core::persistence::{CompleteInstanceParams, EventRecord, Persistence};

use super::SdkBackend;
use crate::error::{Result, SdkError};
use crate::types::{
    CheckpointResult, CustomSignal, InstanceStatus, Signal, SignalType, StatusResponse,
};

/// Embedded backend for SDK operations.
///
/// This backend communicates directly with the persistence layer.
/// Ideal for embedded deployments where runtara-core runs in the same process.
pub struct EmbeddedBackend {
    /// Persistence layer
    persistence: Arc<dyn Persistence>,
    /// Instance ID
    instance_id: String,
    /// Tenant ID
    tenant_id: TenantId,
    /// Tokio runtime for bridging async Persistence trait to sync SDK
    rt: tokio::runtime::Runtime,
}

impl EmbeddedBackend {
    /// Create a new embedded backend.
    ///
    /// # Arguments
    ///
    /// * `persistence` - The persistence layer to use
    /// * `instance_id` - Unique instance identifier
    /// * `tenant_id` - Tenant identifier
    pub fn new(
        persistence: Arc<dyn Persistence>,
        instance_id: impl Into<String>,
        tenant_id: TenantId,
    ) -> Self {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        Self {
            persistence,
            instance_id: instance_id.into(),
            tenant_id,
            rt,
        }
    }

    /// Read the current lifecycle command without consuming its receipt.
    fn take_pending_lifecycle_signal(&self) -> Result<Option<Signal>> {
        let Some(record) = self
            .rt
            .block_on(
                self.persistence
                    .get_pending_signal(&self.tenant_id, &self.instance_id),
            )
            .map_err(|e| SdkError::Internal(e.to_string()))?
        else {
            return Ok(None);
        };
        let signal_type = match record.signal_type {
            CoreSignalType::Cancel => SignalType::Cancel,
            CoreSignalType::Pause => SignalType::Pause,
            CoreSignalType::Shutdown => SignalType::Shutdown,
        };
        Ok(Some(Signal {
            command_id: record.command_id,
            signal_type,
            payload: record.payload.unwrap_or_default(),
            checkpoint_id: None,
        }))
    }
}

impl SdkBackend for EmbeddedBackend {
    fn connect(&self) -> Result<()> {
        // No-op for embedded - we're already "connected"
        debug!("Embedded backend: connect is a no-op");
        Ok(())
    }

    fn is_connected(&self) -> bool {
        // Always connected for embedded
        true
    }

    fn close(&self) {
        // No-op for embedded
        debug!("Embedded backend: close is a no-op");
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(instance_id = %self.instance_id)))]
    fn register(&self, checkpoint_id: Option<&str>) -> Result<()> {
        self.rt
            .block_on(
                self.persistence
                    .register_instance(&self.tenant_id, &self.instance_id),
            )
            .map_err(|e| SdkError::Internal(e.to_string()))?;

        // Update status to running
        self.rt
            .block_on(self.persistence.update_instance_status(
                &self.tenant_id,
                &self.instance_id,
                CoreInstanceStatus::Running,
                Some(Utc::now()),
            ))
            .map_err(|e| SdkError::Internal(e.to_string()))?;

        info!("Instance registered (embedded)");
        Ok(())
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, state), fields(instance_id = %self.instance_id, checkpoint_id = %checkpoint_id, state_size = state.len())))]
    fn checkpoint(&self, checkpoint_id: &str, state: &[u8]) -> Result<CheckpointResult> {
        // Check if checkpoint exists
        let existing = self
            .rt
            .block_on(self.persistence.load_checkpoint(
                &self.tenant_id,
                &self.instance_id,
                checkpoint_id,
            ))
            .map_err(|e| SdkError::Internal(e.to_string()))?;

        if let Some(checkpoint) = existing {
            debug!(
                checkpoint_id = %checkpoint_id,
                "Found existing checkpoint - returning for resume"
            );
            return Ok(CheckpointResult {
                found: true,
                state: checkpoint.state,
                pending_signal: self.take_pending_lifecycle_signal()?,
                custom_signal: None,
            });
        }

        // Save new checkpoint
        self.rt
            .block_on(self.persistence.save_checkpoint(
                &self.tenant_id,
                &self.instance_id,
                checkpoint_id,
                state,
            ))
            .map_err(|e| SdkError::Internal(e.to_string()))?;

        // Update instance's current checkpoint
        self.rt
            .block_on(self.persistence.update_instance_checkpoint(
                &self.tenant_id,
                &self.instance_id,
                checkpoint_id,
            ))
            .map_err(|e| SdkError::Internal(e.to_string()))?;

        debug!(checkpoint_id = %checkpoint_id, "New checkpoint saved");

        Ok(CheckpointResult {
            found: false,
            state: Vec::new(),
            pending_signal: self.take_pending_lifecycle_signal()?,
            custom_signal: None,
        })
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(instance_id = %self.instance_id, checkpoint_id = %checkpoint_id)))]
    fn get_checkpoint(&self, checkpoint_id: &str) -> Result<Option<Vec<u8>>> {
        let result = self
            .rt
            .block_on(self.persistence.load_checkpoint(
                &self.tenant_id,
                &self.instance_id,
                checkpoint_id,
            ))
            .map_err(|e| SdkError::Internal(e.to_string()))?;

        Ok(result.map(|c| c.state))
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(instance_id = %self.instance_id)))]
    fn heartbeat(&self) -> Result<()> {
        let event = EventRecord {
            id: None,
            instance_id: self.instance_id.clone(),
            event_type: CoreEventType::Heartbeat,
            checkpoint_id: None,
            payload: None,
            created_at: Utc::now(),
            subtype: None,
        };

        self.rt
            .block_on(self.persistence.insert_event(&self.tenant_id, &event))
            .map_err(|e| SdkError::Internal(e.to_string()))?;

        debug!("Heartbeat recorded");
        Ok(())
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, output), fields(instance_id = %self.instance_id, output_size = output.len())))]
    fn completed(&self, output: &[u8]) -> Result<()> {
        self.completed_with_label(output, None)
    }

    fn completed_with_label(&self, output: &[u8], run_label: Option<&str>) -> Result<()> {
        let mut params =
            CompleteInstanceParams::new(&self.instance_id, CoreInstanceStatus::Completed)
                .if_running()
                .with_output(output);
        if let Some(label) = run_label {
            params = params.with_run_label(label);
        }
        self.rt
            .block_on(self.persistence.complete_instance(&self.tenant_id, params))
            .map_err(|e| SdkError::Internal(e.to_string()))?;

        let event = EventRecord {
            id: None,
            instance_id: self.instance_id.clone(),
            event_type: CoreEventType::Completed,
            checkpoint_id: None,
            payload: Some(output.to_vec()),
            created_at: Utc::now(),
            subtype: None,
        };

        self.rt
            .block_on(self.persistence.insert_event(&self.tenant_id, &event))
            .map_err(|e| SdkError::Internal(e.to_string()))?;

        info!("Instance completed");
        Ok(())
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(instance_id = %self.instance_id)))]
    fn failed(&self, error: &str) -> Result<()> {
        self.rt
            .block_on(
                self.persistence.complete_instance(
                    &self.tenant_id,
                    CompleteInstanceParams::new(&self.instance_id, CoreInstanceStatus::Failed)
                        .with_error(error),
                ),
            )
            .map_err(|e| SdkError::Internal(e.to_string()))?;

        let event = EventRecord {
            id: None,
            instance_id: self.instance_id.clone(),
            event_type: CoreEventType::Failed,
            checkpoint_id: None,
            payload: Some(error.as_bytes().to_vec()),
            created_at: Utc::now(),
            subtype: None,
        };

        self.rt
            .block_on(self.persistence.insert_event(&self.tenant_id, &event))
            .map_err(|e| SdkError::Internal(e.to_string()))?;

        info!(error = %error, "Instance failed");
        Ok(())
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(instance_id = %self.instance_id)))]
    fn suspended(&self) -> Result<()> {
        self.rt
            .block_on(self.persistence.update_instance_status(
                &self.tenant_id,
                &self.instance_id,
                CoreInstanceStatus::Suspended,
                None,
            ))
            .map_err(|e| SdkError::Internal(e.to_string()))?;

        let event = EventRecord {
            id: None,
            instance_id: self.instance_id.clone(),
            event_type: CoreEventType::Suspended,
            checkpoint_id: None,
            payload: None,
            created_at: Utc::now(),
            subtype: None,
        };

        self.rt
            .block_on(self.persistence.insert_event(&self.tenant_id, &event))
            .map_err(|e| SdkError::Internal(e.to_string()))?;

        info!("Instance suspended");
        Ok(())
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, state), fields(instance_id = %self.instance_id, checkpoint_id = %checkpoint_id)))]
    fn sleep_until(&self, checkpoint_id: &str, wake_at: DateTime<Utc>, state: &[u8]) -> Result<()> {
        // Save checkpoint first
        self.rt
            .block_on(self.persistence.save_checkpoint(
                &self.tenant_id,
                &self.instance_id,
                checkpoint_id,
                state,
            ))
            .map_err(|e| SdkError::Internal(e.to_string()))?;

        // Update checkpoint reference
        self.rt
            .block_on(self.persistence.update_instance_checkpoint(
                &self.tenant_id,
                &self.instance_id,
                checkpoint_id,
            ))
            .map_err(|e| SdkError::Internal(e.to_string()))?;

        // Set sleep_until for wake scheduler
        self.rt
            .block_on(self.persistence.set_instance_sleep(
                &self.tenant_id,
                &self.instance_id,
                wake_at,
            ))
            .map_err(|e| SdkError::Internal(e.to_string()))?;

        // Mark as suspended
        self.rt
            .block_on(self.persistence.update_instance_status(
                &self.tenant_id,
                &self.instance_id,
                CoreInstanceStatus::Suspended,
                None,
            ))
            .map_err(|e| SdkError::Internal(e.to_string()))?;

        // Record the event
        let event = EventRecord {
            id: None,
            instance_id: self.instance_id.clone(),
            event_type: CoreEventType::Suspended,
            checkpoint_id: Some(checkpoint_id.to_string()),
            payload: None,
            created_at: Utc::now(),
            subtype: Some("sleeping".to_string()),
        };

        self.rt
            .block_on(self.persistence.insert_event(&self.tenant_id, &event))
            .map_err(|e| SdkError::Internal(e.to_string()))?;

        info!(wake_at = %wake_at, "Instance sleeping until wake time");
        Ok(())
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, payload), fields(instance_id = %self.instance_id, subtype = %subtype, payload_size = payload.len())))]
    fn send_custom_event(&self, subtype: &str, payload: Vec<u8>) -> Result<()> {
        let event = EventRecord {
            id: None,
            instance_id: self.instance_id.clone(),
            event_type: CoreEventType::Custom,
            checkpoint_id: None,
            payload: Some(payload),
            created_at: Utc::now(),
            subtype: Some(subtype.to_string()),
        };

        self.rt
            .block_on(self.persistence.insert_event(&self.tenant_id, &event))
            .map_err(|e| SdkError::Internal(e.to_string()))?;

        debug!(subtype = %subtype, "Custom event recorded");
        Ok(())
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(instance_id = %self.instance_id, checkpoint_id = %checkpoint_id, attempt = attempt_number)))]
    fn record_retry_attempt(
        &self,
        checkpoint_id: &str,
        attempt_number: u32,
        error_message: Option<&str>,
    ) -> Result<()> {
        self.rt
            .block_on(self.persistence.save_retry_attempt(
                &self.tenant_id,
                &self.instance_id,
                checkpoint_id,
                attempt_number as i32,
                error_message,
            ))
            .map_err(|e| SdkError::Internal(e.to_string()))?;

        debug!(attempt = attempt_number, "Retry attempt recorded");
        Ok(())
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(instance_id = %self.instance_id)))]
    fn get_status(&self) -> Result<StatusResponse> {
        let instance = self
            .rt
            .block_on(
                self.persistence
                    .get_instance(&self.tenant_id, &self.instance_id),
            )
            .map_err(|e| SdkError::Internal(e.to_string()))?;

        match instance {
            Some(record) => {
                let status = sdk_status(record.status);

                Ok(StatusResponse {
                    found: true,
                    status,
                    checkpoint_id: record.checkpoint_id,
                    output: record.output,
                    error: record.error,
                })
            }
            None => Ok(StatusResponse {
                found: false,
                status: InstanceStatus::Pending,
                checkpoint_id: None,
                output: None,
                error: None,
            }),
        }
    }

    fn poll_signals(
        &self,
        checkpoint_id: Option<&str>,
    ) -> Result<(Option<Signal>, Option<CustomSignal>)> {
        let signal = self.take_pending_lifecycle_signal()?;
        let custom = match checkpoint_id {
            Some(id) => self
                .rt
                .block_on(self.persistence.get_custom_signal(
                    &self.tenant_id,
                    &self.instance_id,
                    id,
                ))
                .map_err(|e| SdkError::Internal(e.to_string()))?
                .map(|signal| CustomSignal {
                    signal_id: signal.signal_id,
                    checkpoint_id: signal.checkpoint_id,
                    payload: signal.payload.unwrap_or_default(),
                }),
            None => None,
        };
        Ok((signal, custom))
    }

    fn acknowledge_signal(&self, command_id: &str, signal_type: SignalType) -> Result<bool> {
        let signal_type = match signal_type {
            SignalType::Cancel => CoreSignalType::Cancel,
            SignalType::Pause => CoreSignalType::Pause,
            SignalType::Shutdown => CoreSignalType::Shutdown,
        };
        self.rt
            .block_on(self.persistence.acknowledge_signal(
                &self.tenant_id,
                &self.instance_id,
                command_id,
                signal_type,
            ))
            .map_err(|e| SdkError::Internal(e.to_string()))
    }

    fn get_instance_status(&self, instance_id: &str) -> Result<StatusResponse> {
        let instance = self
            .rt
            .block_on(self.persistence.get_instance(&self.tenant_id, instance_id))
            .map_err(|e| SdkError::Internal(e.to_string()))?;

        match instance {
            Some(record) => {
                let status = sdk_status(record.status);

                Ok(StatusResponse {
                    found: true,
                    status,
                    checkpoint_id: record.checkpoint_id,
                    output: record.output,
                    error: record.error,
                })
            }
            None => Ok(StatusResponse {
                found: false,
                status: InstanceStatus::Pending,
                checkpoint_id: None,
                output: None,
                error: None,
            }),
        }
    }

    fn load_input(&self) -> Result<Option<Vec<u8>>> {
        let instance = self
            .rt
            .block_on(
                self.persistence
                    .get_instance(&self.tenant_id, &self.instance_id),
            )
            .map_err(|e| SdkError::Internal(e.to_string()))?;

        Ok(instance.and_then(|r| r.input))
    }

    fn instance_id(&self) -> &str {
        &self.instance_id
    }

    fn tenant_id(&self) -> &str {
        self.tenant_id.as_str()
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(instance_id = %self.instance_id)))]
    fn set_sleep_until(&self, sleep_until: DateTime<Utc>) -> Result<()> {
        self.rt
            .block_on(self.persistence.set_instance_sleep(
                &self.tenant_id,
                &self.instance_id,
                sleep_until,
            ))
            .map_err(|e| SdkError::Internal(e.to_string()))?;

        debug!(sleep_until = %sleep_until, "Sleep until set");
        Ok(())
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(instance_id = %self.instance_id)))]
    fn clear_sleep(&self) -> Result<()> {
        self.rt
            .block_on(
                self.persistence
                    .clear_instance_sleep(&self.tenant_id, &self.instance_id),
            )
            .map_err(|e| SdkError::Internal(e.to_string()))?;

        debug!("Sleep cleared");
        Ok(())
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(instance_id = %self.instance_id)))]
    fn get_sleep_until(&self) -> Result<Option<DateTime<Utc>>> {
        let instance = self
            .rt
            .block_on(
                self.persistence
                    .get_instance(&self.tenant_id, &self.instance_id),
            )
            .map_err(|e| SdkError::Internal(e.to_string()))?;

        Ok(instance.and_then(|i| i.sleep_until))
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, state), fields(instance_id = %self.instance_id, duration_ms = duration.as_millis() as u64)))]
    fn durable_sleep(&self, duration: Duration, checkpoint_id: &str, state: &[u8]) -> Result<()> {
        let now = Utc::now();
        let wake_at =
            now + chrono::Duration::from_std(duration).unwrap_or(chrono::Duration::zero());

        // Check if we're resuming from a checkpoint
        let checkpoint_result = self.checkpoint(checkpoint_id, state)?;

        if checkpoint_result.found {
            // Resuming - check stored sleep_until time
            let stored_sleep_until = self.get_sleep_until()?;

            if let Some(sleep_until) = stored_sleep_until {
                let now = Utc::now();
                if sleep_until <= now {
                    // Sleep already completed
                    debug!("Sleep already completed, clearing");
                    self.clear_sleep()?;
                    return Ok(());
                }

                // Calculate remaining duration
                let remaining = (sleep_until - now).to_std().unwrap_or(Duration::ZERO);
                info!(
                    remaining_ms = remaining.as_millis() as u64,
                    "Resuming sleep with remaining duration"
                );

                // Sleep for remaining time
                std::thread::sleep(remaining);
                self.clear_sleep()?;
                return Ok(());
            }

            // No sleep_until stored but checkpoint exists - sleep was never started
            // Fall through to set up sleep
        }

        // New sleep - set sleep_until and sleep
        self.set_sleep_until(wake_at)?;
        info!(
            duration_ms = duration.as_millis() as u64,
            "Starting durable sleep"
        );

        std::thread::sleep(duration);
        self.clear_sleep()?;
        info!("Durable sleep completed");

        Ok(())
    }
}

fn sdk_status(value: CoreInstanceStatus) -> InstanceStatus {
    match value {
        CoreInstanceStatus::Pending => InstanceStatus::Pending,
        CoreInstanceStatus::Running => InstanceStatus::Running,
        CoreInstanceStatus::Suspended => InstanceStatus::Suspended,
        CoreInstanceStatus::Completed => InstanceStatus::Completed,
        CoreInstanceStatus::Failed => InstanceStatus::Failed,
        CoreInstanceStatus::Cancelled => InstanceStatus::Cancelled,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtara_core::instance_handlers::mock_persistence::MockPersistence;

    #[test]
    fn cancelled_status_is_preserved() {
        assert_eq!(
            sdk_status(CoreInstanceStatus::Cancelled),
            InstanceStatus::Cancelled
        );
    }

    #[test]
    fn test_embedded_backend_register() {
        let persistence = Arc::new(MockPersistence::new());
        let backend = EmbeddedBackend::new(
            persistence.clone(),
            "test-instance",
            TenantId::new("test-tenant").unwrap(),
        );

        // Connect should be no-op
        backend.connect().unwrap();
        assert!(backend.is_connected());

        // Register
        backend.register(None).unwrap();

        // Verify instance was registered
        let instance = backend
            .rt
            .block_on(
                persistence.get_instance(&TenantId::new("test-tenant").unwrap(), "test-instance"),
            )
            .unwrap()
            .unwrap();
        assert_eq!(instance.instance_id, "test-instance");
        assert_eq!(instance.tenant_id, "test-tenant");
        assert_eq!(instance.status, CoreInstanceStatus::Running);
    }

    #[test]
    fn test_embedded_backend_checkpoint_save() {
        let persistence = Arc::new(MockPersistence::new());
        let backend = EmbeddedBackend::new(
            persistence.clone(),
            "test-instance",
            TenantId::new("test-tenant").unwrap(),
        );

        // Register first
        backend.register(None).unwrap();

        // Save a new checkpoint
        let state = b"test state data";
        let result = backend.checkpoint("step-1", state).unwrap();

        // Should not be found (new checkpoint)
        assert!(!result.found);
        assert!(result.state.is_empty());
        assert!(result.pending_signal.is_none());
    }

    #[test]
    fn checkpoint_propagates_signal_read_errors_on_save_and_replay() {
        let persistence = Arc::new(MockPersistence::new());
        persistence.set_fail_signal_read();
        let backend = EmbeddedBackend::new(
            persistence,
            "test-instance",
            TenantId::new("test-tenant").unwrap(),
        );
        backend.register(None).unwrap();
        for _ in 0..2 {
            let error = backend.checkpoint("step-1", b"state").unwrap_err();
            assert!(error.to_string().contains("injected storage failure"));
        }
    }

    #[test]
    fn test_embedded_backend_checkpoint_resume() {
        let persistence = Arc::new(MockPersistence::new());
        let backend = EmbeddedBackend::new(
            persistence.clone(),
            "test-instance",
            TenantId::new("test-tenant").unwrap(),
        );

        backend.register(None).unwrap();

        // Save a checkpoint
        let state = b"test state data";
        let result = backend.checkpoint("step-1", state).unwrap();
        assert!(!result.found);

        // Try to checkpoint again with same ID - should return existing
        let result2 = backend.checkpoint("step-1", b"new state").unwrap();
        assert!(result2.found);
        assert_eq!(result2.state, state);
    }

    #[test]
    fn test_embedded_backend_get_checkpoint() {
        let persistence = Arc::new(MockPersistence::new());
        let backend = EmbeddedBackend::new(
            persistence.clone(),
            "test-instance",
            TenantId::new("test-tenant").unwrap(),
        );

        backend.register(None).unwrap();

        // Get non-existent checkpoint
        let result = backend.get_checkpoint("step-1").unwrap();
        assert!(result.is_none());

        // Save a checkpoint
        backend.checkpoint("step-1", b"test data").unwrap();

        // Get existing checkpoint
        let result = backend.get_checkpoint("step-1").unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap(), b"test data");
    }

    #[test]
    fn test_embedded_backend_completed() {
        let persistence = Arc::new(MockPersistence::new());
        let backend = EmbeddedBackend::new(
            persistence.clone(),
            "test-instance",
            TenantId::new("test-tenant").unwrap(),
        );

        backend.register(None).unwrap();
        backend.completed(b"result data").unwrap();

        let instance = backend
            .rt
            .block_on(
                persistence.get_instance(&TenantId::new("test-tenant").unwrap(), "test-instance"),
            )
            .unwrap()
            .unwrap();
        assert_eq!(instance.status, CoreInstanceStatus::Completed);
        assert_eq!(instance.output, Some(b"result data".to_vec()));
    }

    #[test]
    fn test_embedded_backend_failed() {
        let persistence = Arc::new(MockPersistence::new());
        let backend = EmbeddedBackend::new(
            persistence.clone(),
            "test-instance",
            TenantId::new("test-tenant").unwrap(),
        );

        backend.register(None).unwrap();
        backend.failed("something went wrong").unwrap();

        let instance = backend
            .rt
            .block_on(
                persistence.get_instance(&TenantId::new("test-tenant").unwrap(), "test-instance"),
            )
            .unwrap()
            .unwrap();
        assert_eq!(instance.status, CoreInstanceStatus::Failed);
        assert_eq!(instance.error, Some("something went wrong".to_string()));
    }

    #[test]
    fn test_embedded_backend_suspended() {
        let persistence = Arc::new(MockPersistence::new());
        let backend = EmbeddedBackend::new(
            persistence.clone(),
            "test-instance",
            TenantId::new("test-tenant").unwrap(),
        );

        backend.register(None).unwrap();
        backend.suspended().unwrap();

        let instance = backend
            .rt
            .block_on(
                persistence.get_instance(&TenantId::new("test-tenant").unwrap(), "test-instance"),
            )
            .unwrap()
            .unwrap();
        assert_eq!(instance.status, CoreInstanceStatus::Suspended);
    }

    #[test]
    fn test_embedded_backend_get_status() {
        let persistence = Arc::new(MockPersistence::new());
        let backend = EmbeddedBackend::new(
            persistence.clone(),
            "test-instance",
            TenantId::new("test-tenant").unwrap(),
        );

        // Get status before registration
        let status = backend.get_status().unwrap();
        assert!(!status.found);

        // Register and get status
        backend.register(None).unwrap();
        let status = backend.get_status().unwrap();
        assert!(status.found);
        assert_eq!(status.status, crate::types::InstanceStatus::Running);
    }

    #[test]
    fn test_embedded_backend_ids() {
        let persistence = Arc::new(MockPersistence::new());
        let backend = EmbeddedBackend::new(
            persistence,
            "my-instance",
            TenantId::new("my-tenant").unwrap(),
        );

        assert_eq!(backend.instance_id(), "my-instance");
        assert_eq!(backend.tenant_id(), "my-tenant");
    }
}
