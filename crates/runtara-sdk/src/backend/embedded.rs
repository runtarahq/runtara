// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Embedded SDK backend for direct database access.
//!
//! This backend bypasses QUIC and calls the persistence layer directly,
//! suitable for embedding runtara-core within the same process.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use runtara_core::persistence::{EventRecord, Persistence};
use tracing::{debug, info, instrument};

use super::SdkBackend;
use crate::error::{Result, SdkError};
use crate::types::{CheckpointResult, InstanceStatus, StatusResponse};

/// Embedded backend for SDK operations.
///
/// This backend communicates directly with the persistence layer,
/// bypassing the QUIC transport. Ideal for embedded deployments
/// where runtara-core runs in the same process.
pub struct EmbeddedBackend {
    /// Persistence layer
    persistence: Arc<dyn Persistence>,
    /// Instance ID
    instance_id: String,
    /// Tenant ID
    tenant_id: String,
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
        tenant_id: impl Into<String>,
    ) -> Self {
        Self {
            persistence,
            instance_id: instance_id.into(),
            tenant_id: tenant_id.into(),
        }
    }
}

#[async_trait]
impl SdkBackend for EmbeddedBackend {
    #[cfg(feature = "quic")]
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn connect(&self) -> Result<()> {
        // No-op for embedded - we're already "connected"
        debug!("Embedded backend: connect is a no-op");
        Ok(())
    }

    async fn is_connected(&self) -> bool {
        // Always connected for embedded
        true
    }

    async fn close(&self) {
        // No-op for embedded
        debug!("Embedded backend: close is a no-op");
    }

    #[instrument(skip(self), fields(instance_id = %self.instance_id))]
    async fn register(&self, _checkpoint_id: Option<&str>) -> Result<()> {
        self.persistence
            .register_instance(&self.instance_id, &self.tenant_id)
            .await
            .map_err(|e| SdkError::Internal(e.to_string()))?;

        // Update status to running
        self.persistence
            .update_instance_status(&self.instance_id, "running", Some(Utc::now()))
            .await
            .map_err(|e| SdkError::Internal(e.to_string()))?;

        info!("Instance registered (embedded)");
        Ok(())
    }

    #[instrument(skip(self, state), fields(instance_id = %self.instance_id, checkpoint_id = %checkpoint_id, state_size = state.len()))]
    async fn checkpoint(&self, checkpoint_id: &str, state: &[u8]) -> Result<CheckpointResult> {
        // Check if checkpoint exists
        let existing = self
            .persistence
            .load_checkpoint(&self.instance_id, checkpoint_id)
            .await
            .map_err(|e| SdkError::Internal(e.to_string()))?;

        if let Some(checkpoint) = existing {
            debug!(
                checkpoint_id = %checkpoint_id,
                "Found existing checkpoint - returning for resume"
            );
            return Ok(CheckpointResult {
                found: true,
                state: checkpoint.state,
                pending_signal: None, // No signal support in embedded mode
                custom_signal: None,
            });
        }

        // Save new checkpoint
        self.persistence
            .save_checkpoint(&self.instance_id, checkpoint_id, state)
            .await
            .map_err(|e| SdkError::Internal(e.to_string()))?;

        // Update instance's current checkpoint
        self.persistence
            .update_instance_checkpoint(&self.instance_id, checkpoint_id)
            .await
            .map_err(|e| SdkError::Internal(e.to_string()))?;

        debug!(checkpoint_id = %checkpoint_id, "New checkpoint saved");

        Ok(CheckpointResult {
            found: false,
            state: Vec::new(),
            pending_signal: None,
            custom_signal: None,
        })
    }

    #[instrument(skip(self), fields(instance_id = %self.instance_id, checkpoint_id = %checkpoint_id))]
    async fn get_checkpoint(&self, checkpoint_id: &str) -> Result<Option<Vec<u8>>> {
        let result = self
            .persistence
            .load_checkpoint(&self.instance_id, checkpoint_id)
            .await
            .map_err(|e| SdkError::Internal(e.to_string()))?;

        Ok(result.map(|c| c.state))
    }

    #[instrument(skip(self), fields(instance_id = %self.instance_id))]
    async fn heartbeat(&self) -> Result<()> {
        let event = EventRecord {
            id: None,
            instance_id: self.instance_id.clone(),
            event_type: "heartbeat".to_string(),
            checkpoint_id: None,
            payload: None,
            created_at: Utc::now(),
            subtype: None,
        };

        self.persistence
            .insert_event(&event)
            .await
            .map_err(|e| SdkError::Internal(e.to_string()))?;

        debug!("Heartbeat recorded");
        Ok(())
    }

    #[instrument(skip(self, output), fields(instance_id = %self.instance_id, output_size = output.len()))]
    async fn completed(&self, output: &[u8]) -> Result<()> {
        self.persistence
            .complete_instance(&self.instance_id, Some(output), None)
            .await
            .map_err(|e| SdkError::Internal(e.to_string()))?;

        let event = EventRecord {
            id: None,
            instance_id: self.instance_id.clone(),
            event_type: "completed".to_string(),
            checkpoint_id: None,
            payload: Some(output.to_vec()),
            created_at: Utc::now(),
            subtype: None,
        };

        self.persistence
            .insert_event(&event)
            .await
            .map_err(|e| SdkError::Internal(e.to_string()))?;

        info!("Instance completed");
        Ok(())
    }

    #[instrument(skip(self), fields(instance_id = %self.instance_id))]
    async fn failed(&self, error: &str) -> Result<()> {
        self.persistence
            .complete_instance(&self.instance_id, None, Some(error))
            .await
            .map_err(|e| SdkError::Internal(e.to_string()))?;

        let event = EventRecord {
            id: None,
            instance_id: self.instance_id.clone(),
            event_type: "failed".to_string(),
            checkpoint_id: None,
            payload: Some(error.as_bytes().to_vec()),
            created_at: Utc::now(),
            subtype: None,
        };

        self.persistence
            .insert_event(&event)
            .await
            .map_err(|e| SdkError::Internal(e.to_string()))?;

        info!(error = %error, "Instance failed");
        Ok(())
    }

    #[instrument(skip(self), fields(instance_id = %self.instance_id))]
    async fn suspended(&self) -> Result<()> {
        self.persistence
            .update_instance_status(&self.instance_id, "suspended", None)
            .await
            .map_err(|e| SdkError::Internal(e.to_string()))?;

        let event = EventRecord {
            id: None,
            instance_id: self.instance_id.clone(),
            event_type: "suspended".to_string(),
            checkpoint_id: None,
            payload: None,
            created_at: Utc::now(),
            subtype: None,
        };

        self.persistence
            .insert_event(&event)
            .await
            .map_err(|e| SdkError::Internal(e.to_string()))?;

        info!("Instance suspended");
        Ok(())
    }

    #[instrument(skip(self, state), fields(instance_id = %self.instance_id, checkpoint_id = %checkpoint_id))]
    async fn sleep_until(
        &self,
        checkpoint_id: &str,
        wake_at: DateTime<Utc>,
        state: &[u8],
    ) -> Result<()> {
        // Save checkpoint first
        self.persistence
            .save_checkpoint(&self.instance_id, checkpoint_id, state)
            .await
            .map_err(|e| SdkError::Internal(e.to_string()))?;

        // Update checkpoint reference
        self.persistence
            .update_instance_checkpoint(&self.instance_id, checkpoint_id)
            .await
            .map_err(|e| SdkError::Internal(e.to_string()))?;

        // Set sleep_until for wake scheduler
        self.persistence
            .set_instance_sleep(&self.instance_id, wake_at)
            .await
            .map_err(|e| SdkError::Internal(e.to_string()))?;

        // Mark as suspended
        self.persistence
            .update_instance_status(&self.instance_id, "suspended", None)
            .await
            .map_err(|e| SdkError::Internal(e.to_string()))?;

        // Record the event
        let event = EventRecord {
            id: None,
            instance_id: self.instance_id.clone(),
            event_type: "suspended".to_string(),
            checkpoint_id: Some(checkpoint_id.to_string()),
            payload: None,
            created_at: Utc::now(),
            subtype: Some("sleeping".to_string()),
        };

        self.persistence
            .insert_event(&event)
            .await
            .map_err(|e| SdkError::Internal(e.to_string()))?;

        info!(wake_at = %wake_at, "Instance sleeping until wake time");
        Ok(())
    }

    #[instrument(skip(self, payload), fields(instance_id = %self.instance_id, subtype = %subtype, payload_size = payload.len()))]
    async fn send_custom_event(&self, subtype: &str, payload: Vec<u8>) -> Result<()> {
        let event = EventRecord {
            id: None,
            instance_id: self.instance_id.clone(),
            event_type: "custom".to_string(),
            checkpoint_id: None,
            payload: Some(payload),
            created_at: Utc::now(),
            subtype: Some(subtype.to_string()),
        };

        self.persistence
            .insert_event(&event)
            .await
            .map_err(|e| SdkError::Internal(e.to_string()))?;

        debug!(subtype = %subtype, "Custom event recorded");
        Ok(())
    }

    #[instrument(skip(self), fields(instance_id = %self.instance_id, checkpoint_id = %checkpoint_id, attempt = attempt_number))]
    async fn record_retry_attempt(
        &self,
        checkpoint_id: &str,
        attempt_number: u32,
        error_message: Option<&str>,
    ) -> Result<()> {
        self.persistence
            .save_retry_attempt(
                &self.instance_id,
                checkpoint_id,
                attempt_number as i32,
                error_message,
            )
            .await
            .map_err(|e| SdkError::Internal(e.to_string()))?;

        debug!(attempt = attempt_number, "Retry attempt recorded");
        Ok(())
    }

    #[instrument(skip(self), fields(instance_id = %self.instance_id))]
    async fn get_status(&self) -> Result<StatusResponse> {
        let instance = self
            .persistence
            .get_instance(&self.instance_id)
            .await
            .map_err(|e| SdkError::Internal(e.to_string()))?;

        match instance {
            Some(record) => {
                let status = match record.status.as_str() {
                    "pending" => InstanceStatus::Pending,
                    "running" => InstanceStatus::Running,
                    "suspended" => InstanceStatus::Suspended,
                    "completed" => InstanceStatus::Completed,
                    "failed" => InstanceStatus::Failed,
                    _ => InstanceStatus::Pending,
                };

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

    fn instance_id(&self) -> &str {
        &self.instance_id
    }

    fn tenant_id(&self) -> &str {
        &self.tenant_id
    }

    #[instrument(skip(self), fields(instance_id = %self.instance_id))]
    async fn set_sleep_until(&self, sleep_until: DateTime<Utc>) -> Result<()> {
        self.persistence
            .set_instance_sleep(&self.instance_id, sleep_until)
            .await
            .map_err(|e| SdkError::Internal(e.to_string()))?;

        debug!(sleep_until = %sleep_until, "Sleep until set");
        Ok(())
    }

    #[instrument(skip(self), fields(instance_id = %self.instance_id))]
    async fn clear_sleep(&self) -> Result<()> {
        self.persistence
            .clear_instance_sleep(&self.instance_id)
            .await
            .map_err(|e| SdkError::Internal(e.to_string()))?;

        debug!("Sleep cleared");
        Ok(())
    }

    #[instrument(skip(self), fields(instance_id = %self.instance_id))]
    async fn get_sleep_until(&self) -> Result<Option<DateTime<Utc>>> {
        let instance = self
            .persistence
            .get_instance(&self.instance_id)
            .await
            .map_err(|e| SdkError::Internal(e.to_string()))?;

        Ok(instance.and_then(|i| i.sleep_until))
    }

    #[instrument(skip(self, state), fields(instance_id = %self.instance_id, duration_ms = duration.as_millis() as u64))]
    async fn durable_sleep(
        &self,
        duration: Duration,
        checkpoint_id: &str,
        state: &[u8],
    ) -> Result<()> {
        let now = Utc::now();
        let wake_at =
            now + chrono::Duration::from_std(duration).unwrap_or(chrono::Duration::zero());

        // Check if we're resuming from a checkpoint
        let checkpoint_result = self.checkpoint(checkpoint_id, state).await?;

        if checkpoint_result.found {
            // Resuming - check stored sleep_until time
            let stored_sleep_until = self.get_sleep_until().await?;

            if let Some(sleep_until) = stored_sleep_until {
                let now = Utc::now();
                if sleep_until <= now {
                    // Sleep already completed
                    debug!("Sleep already completed, clearing");
                    self.clear_sleep().await?;
                    return Ok(());
                }

                // Calculate remaining duration
                let remaining = (sleep_until - now).to_std().unwrap_or(Duration::ZERO);
                info!(
                    remaining_ms = remaining.as_millis() as u64,
                    "Resuming sleep with remaining duration"
                );

                // Sleep for remaining time
                tokio::time::sleep(remaining).await;
                self.clear_sleep().await?;
                return Ok(());
            }

            // No sleep_until stored but checkpoint exists - sleep was never started
            // Fall through to set up sleep
        }

        // New sleep - set sleep_until and sleep
        self.set_sleep_until(wake_at).await?;
        info!(
            duration_ms = duration.as_millis() as u64,
            "Starting durable sleep"
        );

        tokio::time::sleep(duration).await;
        self.clear_sleep().await?;
        info!("Durable sleep completed");

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtara_core::error::CoreError;

    // Use std::result::Result to avoid conflict with SDK's Result type alias
    type CoreResult<T> = std::result::Result<T, CoreError>;

    // Mock persistence for testing (in-memory)
    struct MockPersistence {
        instances: tokio::sync::RwLock<std::collections::HashMap<String, MockInstance>>,
        checkpoints: tokio::sync::RwLock<std::collections::HashMap<String, Vec<u8>>>,
    }

    struct MockInstance {
        #[allow(dead_code)]
        instance_id: String,
        #[allow(dead_code)]
        tenant_id: String,
        status: String,
        checkpoint_id: Option<String>,
        output: Option<Vec<u8>>,
        error: Option<String>,
        sleep_until: Option<DateTime<Utc>>,
    }

    impl MockPersistence {
        fn new() -> Self {
            Self {
                instances: tokio::sync::RwLock::new(std::collections::HashMap::new()),
                checkpoints: tokio::sync::RwLock::new(std::collections::HashMap::new()),
            }
        }

        fn checkpoint_key(instance_id: &str, checkpoint_id: &str) -> String {
            format!("{}:{}", instance_id, checkpoint_id)
        }
    }

    #[async_trait]
    impl Persistence for MockPersistence {
        async fn register_instance(&self, instance_id: &str, tenant_id: &str) -> CoreResult<()> {
            let mut instances = self.instances.write().await;
            instances.insert(
                instance_id.to_string(),
                MockInstance {
                    instance_id: instance_id.to_string(),
                    tenant_id: tenant_id.to_string(),
                    status: "pending".to_string(),
                    checkpoint_id: None,
                    output: None,
                    error: None,
                    sleep_until: None,
                },
            );
            Ok(())
        }

        async fn get_instance(
            &self,
            instance_id: &str,
        ) -> CoreResult<Option<runtara_core::persistence::InstanceRecord>> {
            let instances = self.instances.read().await;
            Ok(instances
                .get(instance_id)
                .map(|inst| runtara_core::persistence::InstanceRecord {
                    instance_id: instance_id.to_string(),
                    tenant_id: inst.tenant_id.clone(),
                    definition_version: 1,
                    status: inst.status.clone(),
                    checkpoint_id: inst.checkpoint_id.clone(),
                    attempt: 1,
                    max_attempts: 3,
                    created_at: chrono::Utc::now(),
                    started_at: Some(chrono::Utc::now()),
                    finished_at: None,
                    output: inst.output.clone(),
                    error: inst.error.clone(),
                    sleep_until: inst.sleep_until,
                }))
        }

        async fn update_instance_status(
            &self,
            instance_id: &str,
            status: &str,
            _started_at: Option<chrono::DateTime<chrono::Utc>>,
        ) -> CoreResult<()> {
            let mut instances = self.instances.write().await;
            if let Some(inst) = instances.get_mut(instance_id) {
                inst.status = status.to_string();
            }
            Ok(())
        }

        async fn update_instance_checkpoint(
            &self,
            instance_id: &str,
            checkpoint_id: &str,
        ) -> CoreResult<()> {
            let mut instances = self.instances.write().await;
            if let Some(inst) = instances.get_mut(instance_id) {
                inst.checkpoint_id = Some(checkpoint_id.to_string());
            }
            Ok(())
        }

        async fn complete_instance(
            &self,
            instance_id: &str,
            output: Option<&[u8]>,
            error: Option<&str>,
        ) -> CoreResult<()> {
            let mut instances = self.instances.write().await;
            if let Some(inst) = instances.get_mut(instance_id) {
                if error.is_some() {
                    inst.status = "failed".to_string();
                    inst.error = error.map(|e| e.to_string());
                } else {
                    inst.status = "completed".to_string();
                    inst.output = output.map(|o| o.to_vec());
                }
            }
            Ok(())
        }

        async fn save_checkpoint(
            &self,
            instance_id: &str,
            checkpoint_id: &str,
            state: &[u8],
        ) -> CoreResult<()> {
            let mut checkpoints = self.checkpoints.write().await;
            let key = Self::checkpoint_key(instance_id, checkpoint_id);
            checkpoints.insert(key, state.to_vec());
            Ok(())
        }

        async fn load_checkpoint(
            &self,
            instance_id: &str,
            checkpoint_id: &str,
        ) -> CoreResult<Option<runtara_core::persistence::CheckpointRecord>> {
            let checkpoints = self.checkpoints.read().await;
            let key = Self::checkpoint_key(instance_id, checkpoint_id);
            Ok(checkpoints
                .get(&key)
                .map(|state| runtara_core::persistence::CheckpointRecord {
                    id: 1,
                    instance_id: instance_id.to_string(),
                    checkpoint_id: checkpoint_id.to_string(),
                    state: state.clone(),
                    created_at: chrono::Utc::now(),
                    is_compensatable: false,
                    compensation_step_id: None,
                    compensation_data: None,
                    compensation_state: None,
                    compensation_order: 0,
                }))
        }

        async fn list_checkpoints(
            &self,
            _instance_id: &str,
            _checkpoint_id: Option<&str>,
            _limit: i64,
            _offset: i64,
            _created_after: Option<chrono::DateTime<chrono::Utc>>,
            _created_before: Option<chrono::DateTime<chrono::Utc>>,
        ) -> CoreResult<Vec<runtara_core::persistence::CheckpointRecord>> {
            Ok(vec![])
        }

        async fn count_checkpoints(
            &self,
            _instance_id: &str,
            _checkpoint_id: Option<&str>,
            _created_after: Option<chrono::DateTime<chrono::Utc>>,
            _created_before: Option<chrono::DateTime<chrono::Utc>>,
        ) -> CoreResult<i64> {
            Ok(0)
        }

        async fn insert_event(
            &self,
            _event: &runtara_core::persistence::EventRecord,
        ) -> CoreResult<()> {
            Ok(())
        }

        async fn insert_signal(
            &self,
            _instance_id: &str,
            _signal_type: &str,
            _payload: &[u8],
        ) -> CoreResult<()> {
            Ok(())
        }

        async fn get_pending_signal(
            &self,
            _instance_id: &str,
        ) -> CoreResult<Option<runtara_core::persistence::SignalRecord>> {
            Ok(None)
        }

        async fn acknowledge_signal(&self, _instance_id: &str) -> CoreResult<()> {
            Ok(())
        }

        async fn insert_custom_signal(
            &self,
            _instance_id: &str,
            _checkpoint_id: &str,
            _payload: &[u8],
        ) -> CoreResult<()> {
            Ok(())
        }

        async fn take_pending_custom_signal(
            &self,
            _instance_id: &str,
            _checkpoint_id: &str,
        ) -> CoreResult<Option<runtara_core::persistence::CustomSignalRecord>> {
            Ok(None)
        }

        async fn save_retry_attempt(
            &self,
            _instance_id: &str,
            _checkpoint_id: &str,
            _attempt: i32,
            _error_message: Option<&str>,
        ) -> CoreResult<()> {
            Ok(())
        }

        async fn list_instances(
            &self,
            _tenant_id: Option<&str>,
            _status: Option<&str>,
            _limit: i64,
            _offset: i64,
        ) -> CoreResult<Vec<runtara_core::persistence::InstanceRecord>> {
            Ok(vec![])
        }

        async fn health_check_db(&self) -> CoreResult<bool> {
            Ok(true)
        }

        async fn count_active_instances(&self) -> CoreResult<i64> {
            Ok(0)
        }

        async fn set_instance_sleep(
            &self,
            instance_id: &str,
            sleep_until: DateTime<Utc>,
        ) -> CoreResult<()> {
            let mut instances = self.instances.write().await;
            if let Some(inst) = instances.get_mut(instance_id) {
                inst.sleep_until = Some(sleep_until);
            }
            Ok(())
        }

        async fn clear_instance_sleep(&self, instance_id: &str) -> CoreResult<()> {
            let mut instances = self.instances.write().await;
            if let Some(inst) = instances.get_mut(instance_id) {
                inst.sleep_until = None;
            }
            Ok(())
        }

        async fn get_sleeping_instances_due(
            &self,
            _limit: i64,
        ) -> CoreResult<Vec<runtara_core::persistence::InstanceRecord>> {
            Ok(vec![])
        }

        async fn list_events(
            &self,
            _instance_id: &str,
            _filter: &runtara_core::persistence::ListEventsFilter,
            _limit: i64,
            _offset: i64,
        ) -> CoreResult<Vec<runtara_core::persistence::EventRecord>> {
            Ok(vec![])
        }

        async fn count_events(
            &self,
            _instance_id: &str,
            _filter: &runtara_core::persistence::ListEventsFilter,
        ) -> CoreResult<i64> {
            Ok(0)
        }
    }

    #[tokio::test]
    async fn test_embedded_backend_register() {
        let persistence = Arc::new(MockPersistence::new());
        let backend = EmbeddedBackend::new(persistence.clone(), "test-instance", "test-tenant");

        // Connect should be no-op
        backend.connect().await.unwrap();
        assert!(backend.is_connected().await);

        // Register
        backend.register(None).await.unwrap();

        // Verify instance was registered
        let instance = persistence
            .get_instance("test-instance")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(instance.instance_id, "test-instance");
        assert_eq!(instance.tenant_id, "test-tenant");
        assert_eq!(instance.status, "running");
    }

    #[tokio::test]
    async fn test_embedded_backend_checkpoint_save() {
        let persistence = Arc::new(MockPersistence::new());
        let backend = EmbeddedBackend::new(persistence.clone(), "test-instance", "test-tenant");

        // Register first
        backend.register(None).await.unwrap();

        // Save a new checkpoint
        let state = b"test state data";
        let result = backend.checkpoint("step-1", state).await.unwrap();

        // Should not be found (new checkpoint)
        assert!(!result.found);
        assert!(result.state.is_empty());
        assert!(result.pending_signal.is_none());
    }

    #[tokio::test]
    async fn test_embedded_backend_checkpoint_resume() {
        let persistence = Arc::new(MockPersistence::new());
        let backend = EmbeddedBackend::new(persistence.clone(), "test-instance", "test-tenant");

        backend.register(None).await.unwrap();

        // Save a checkpoint
        let state = b"test state data";
        let result = backend.checkpoint("step-1", state).await.unwrap();
        assert!(!result.found);

        // Try to checkpoint again with same ID - should return existing
        let result2 = backend.checkpoint("step-1", b"new state").await.unwrap();
        assert!(result2.found);
        assert_eq!(result2.state, state);
    }

    #[tokio::test]
    async fn test_embedded_backend_get_checkpoint() {
        let persistence = Arc::new(MockPersistence::new());
        let backend = EmbeddedBackend::new(persistence.clone(), "test-instance", "test-tenant");

        backend.register(None).await.unwrap();

        // Get non-existent checkpoint
        let result = backend.get_checkpoint("step-1").await.unwrap();
        assert!(result.is_none());

        // Save a checkpoint
        backend.checkpoint("step-1", b"test data").await.unwrap();

        // Get existing checkpoint
        let result = backend.get_checkpoint("step-1").await.unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap(), b"test data");
    }

    #[tokio::test]
    async fn test_embedded_backend_completed() {
        let persistence = Arc::new(MockPersistence::new());
        let backend = EmbeddedBackend::new(persistence.clone(), "test-instance", "test-tenant");

        backend.register(None).await.unwrap();
        backend.completed(b"result data").await.unwrap();

        let instance = persistence
            .get_instance("test-instance")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(instance.status, "completed");
        assert_eq!(instance.output, Some(b"result data".to_vec()));
    }

    #[tokio::test]
    async fn test_embedded_backend_failed() {
        let persistence = Arc::new(MockPersistence::new());
        let backend = EmbeddedBackend::new(persistence.clone(), "test-instance", "test-tenant");

        backend.register(None).await.unwrap();
        backend.failed("something went wrong").await.unwrap();

        let instance = persistence
            .get_instance("test-instance")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(instance.status, "failed");
        assert_eq!(instance.error, Some("something went wrong".to_string()));
    }

    #[tokio::test]
    async fn test_embedded_backend_suspended() {
        let persistence = Arc::new(MockPersistence::new());
        let backend = EmbeddedBackend::new(persistence.clone(), "test-instance", "test-tenant");

        backend.register(None).await.unwrap();
        backend.suspended().await.unwrap();

        let instance = persistence
            .get_instance("test-instance")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(instance.status, "suspended");
    }

    #[tokio::test]
    async fn test_embedded_backend_get_status() {
        let persistence = Arc::new(MockPersistence::new());
        let backend = EmbeddedBackend::new(persistence.clone(), "test-instance", "test-tenant");

        // Get status before registration
        let status = backend.get_status().await.unwrap();
        assert!(!status.found);

        // Register and get status
        backend.register(None).await.unwrap();
        let status = backend.get_status().await.unwrap();
        assert!(status.found);
        assert_eq!(status.status, crate::types::InstanceStatus::Running);
    }

    #[tokio::test]
    async fn test_embedded_backend_ids() {
        let persistence = Arc::new(MockPersistence::new());
        let backend = EmbeddedBackend::new(persistence, "my-instance", "my-tenant");

        assert_eq!(backend.instance_id(), "my-instance");
        assert_eq!(backend.tenant_id(), "my-tenant");
    }
}
