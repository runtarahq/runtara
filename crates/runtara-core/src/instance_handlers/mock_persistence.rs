// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Handler fixtures and fault injection over the tenant-enforcing reference backend.

use crate::TenantId;
use crate::domain::InstanceStatus as CoreInstanceStatus;
use crate::domain::{InstanceStatus, SignalType};
use crate::error::CoreError;
use crate::persistence::memory::InMemoryPersistence;
use crate::persistence::*;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::sync::Mutex;

/// Test backend with explicit fault injection and reference storage semantics.
#[derive(Default)]
pub struct MockPersistence {
    inner: InMemoryPersistence,
    fail_register: Mutex<bool>,
    fail_status_update: Mutex<bool>,
    fail_signal_read: Mutex<bool>,
    fail_instance_read: Mutex<bool>,
    fail_count: Mutex<bool>,
    fail_lifecycle: Mutex<bool>,
    remove_parent_before_event: Mutex<bool>,
    active_instance_count: Mutex<Option<i64>>,
}

impl MockPersistence {
    /// Create an empty backend.
    pub fn new() -> Self {
        Self::default()
    }
    /// Override the admission count for handler tests.
    pub fn with_active_count(self, count: i64) -> Self {
        *self.active_instance_count.lock().unwrap() = Some(count);
        self
    }
    /// Seed an instance fixture.
    pub fn with_instance(self, instance: InstanceRecord) -> Self {
        self.inner.seed_instance(instance);
        self
    }
    /// Seed a checkpoint fixture.
    pub fn with_checkpoint(self, checkpoint: CheckpointRecord) -> Self {
        self.inner.seed_checkpoint(checkpoint);
        self
    }
    /// Seed a lifecycle signal fixture.
    pub fn with_signal(self, signal: SignalRecord) -> Self {
        self.inner.seed_signal(signal);
        self
    }
    /// Seed a custom signal fixture.
    pub fn with_custom_signal(self, signal: CustomSignalRecord) -> Self {
        self.inner.seed_custom_signal(signal);
        self
    }
    /// Fail subsequent registrations.
    pub fn set_fail_register(&self) {
        *self.fail_register.lock().unwrap() = true;
    }
    /// Fail subsequent status updates.
    pub fn set_fail_status_update(&self) {
        *self.fail_status_update.lock().unwrap() = true;
    }
    /// Fail subsequent signal reads.
    pub fn set_fail_signal_read(&self) {
        *self.fail_signal_read.lock().unwrap() = true;
    }
    /// Fail subsequent instance reads.
    pub fn set_fail_instance_read(&self) {
        *self.fail_instance_read.lock().unwrap() = true;
    }
    /// Fail subsequent admission counts.
    pub fn set_fail_count(&self) {
        *self.fail_count.lock().unwrap() = true;
    }
    /// Fail lifecycle command application before any mutation.
    pub fn set_fail_lifecycle(&self) {
        *self.fail_lifecycle.lock().unwrap() = true;
    }
    /// Simulate a parent disappearing between a handler's read and event write.
    pub fn set_remove_parent_before_event(&self) {
        *self.remove_parent_before_event.lock().unwrap() = true;
    }
    /// Inspect all recorded events in this test fixture.
    pub fn get_events(&self) -> Vec<EventRecord> {
        self.inner.recorded_events()
    }

    fn fail_if(&self, flag: &Mutex<bool>, operation: &str) -> Result<(), CoreError> {
        if *flag.lock().unwrap() {
            Err(CoreError::PersistenceError {
                operation: operation.into(),
                details: "injected storage failure".into(),
            })
        } else {
            Ok(())
        }
    }
}

/// Build an `InstanceRecord` with plausible defaults for everything the
/// caller does not care about.
pub fn make_instance(
    tenant_id: &str,
    instance_id: &str,
    status: CoreInstanceStatus,
) -> InstanceRecord {
    InstanceRecord {
        run_label: None,
        instance_id: instance_id.to_string(),
        tenant_id: tenant_id.to_string(),
        definition_version: 1,
        status,
        checkpoint_id: None,
        attempt: 1,
        max_attempts: 3,
        created_at: Utc::now(),
        started_at: None,
        finished_at: None,
        input: None,
        output: None,
        error: None,
        sleep_until: None,
        wake_reason: None,
        termination_reason: None,
        exit_code: None,
        recovery_attempts: 0,
        recovery_marker: None,
    }
}

/// Build a `CheckpointRecord` holding `state`.
pub fn make_checkpoint(instance_id: &str, checkpoint_id: &str, state: &[u8]) -> CheckpointRecord {
    CheckpointRecord {
        instance_id: instance_id.to_string(),
        checkpoint_id: checkpoint_id.to_string(),
        state: state.to_vec(),
        created_at: Utc::now(),
    }
}

/// Build an unacknowledged `SignalRecord` with no payload.
pub fn make_signal(instance_id: &str, signal_type: crate::domain::SignalType) -> SignalRecord {
    SignalRecord {
        command_id: uuid::Uuid::new_v4().to_string(),
        instance_id: instance_id.to_string(),
        signal_type,
        payload: None,
        created_at: Utc::now(),
        acknowledged_at: None,
    }
}

#[async_trait]
impl Persistence for MockPersistence {
    async fn register_instance(
        &self,
        tenant_id: &TenantId,
        instance_id: &str,
    ) -> Result<(), CoreError> {
        self.fail_if(&self.fail_register, "register_instance")?;
        self.inner.register_instance(tenant_id, instance_id).await
    }
    async fn try_register_instance(
        &self,
        tenant_id: &TenantId,
        instance_id: &str,
        input: Option<&[u8]>,
    ) -> Result<bool, CoreError> {
        self.fail_if(&self.fail_register, "register_instance")?;
        self.inner
            .try_register_instance(tenant_id, instance_id, input)
            .await
    }
    async fn get_instance(
        &self,
        tenant_id: &TenantId,
        instance_id: &str,
    ) -> Result<Option<InstanceRecord>, CoreError> {
        self.fail_if(&self.fail_instance_read, "get_instance")?;
        self.inner.get_instance(tenant_id, instance_id).await
    }
    async fn get_instance_meta(
        &self,
        tenant_id: &TenantId,
        instance_id: &str,
    ) -> Result<Option<InstanceRecord>, CoreError> {
        self.fail_if(&self.fail_instance_read, "get_instance")?;
        self.inner.get_instance_meta(tenant_id, instance_id).await
    }
    async fn update_instance_status(
        &self,
        tenant_id: &TenantId,
        instance_id: &str,
        status: InstanceStatus,
        started_at: Option<DateTime<Utc>>,
    ) -> Result<(), CoreError> {
        self.fail_if(&self.fail_status_update, "update_instance_status")?;
        self.inner
            .update_instance_status(tenant_id, instance_id, status, started_at)
            .await
    }
    async fn update_instance_checkpoint(
        &self,
        tenant_id: &TenantId,
        instance_id: &str,
        checkpoint_id: &str,
    ) -> Result<(), CoreError> {
        self.inner
            .update_instance_checkpoint(tenant_id, instance_id, checkpoint_id)
            .await
    }
    async fn complete_instance(
        &self,
        tenant_id: &TenantId,
        params: CompleteInstanceParams<'_>,
    ) -> Result<bool, CoreError> {
        self.inner.complete_instance(tenant_id, params).await
    }
    async fn store_instance_input(
        &self,
        tenant_id: &TenantId,
        instance_id: &str,
        input: &[u8],
    ) -> Result<(), CoreError> {
        self.inner
            .store_instance_input(tenant_id, instance_id, input)
            .await
    }
    async fn save_checkpoint(
        &self,
        tenant_id: &TenantId,
        instance_id: &str,
        checkpoint_id: &str,
        state: &[u8],
    ) -> Result<(), CoreError> {
        self.inner
            .save_checkpoint(tenant_id, instance_id, checkpoint_id, state)
            .await
    }
    async fn load_checkpoint(
        &self,
        tenant_id: &TenantId,
        instance_id: &str,
        checkpoint_id: &str,
    ) -> Result<Option<CheckpointRecord>, CoreError> {
        self.inner
            .load_checkpoint(tenant_id, instance_id, checkpoint_id)
            .await
    }
    #[allow(clippy::too_many_arguments)] // Existing filters plus explicit tenant scope.
    async fn list_checkpoints(
        &self,
        tenant_id: &TenantId,
        instance_id: &str,
        checkpoint_id: Option<&str>,
        limit: i64,
        offset: i64,
        created_after: Option<DateTime<Utc>>,
        created_before: Option<DateTime<Utc>>,
    ) -> Result<Vec<CheckpointRecord>, CoreError> {
        self.inner
            .list_checkpoints(
                tenant_id,
                instance_id,
                checkpoint_id,
                limit,
                offset,
                created_after,
                created_before,
            )
            .await
    }
    async fn count_checkpoints(
        &self,
        tenant_id: &TenantId,
        instance_id: &str,
        checkpoint_id: Option<&str>,
        created_after: Option<DateTime<Utc>>,
        created_before: Option<DateTime<Utc>>,
    ) -> Result<i64, CoreError> {
        self.inner
            .count_checkpoints(
                tenant_id,
                instance_id,
                checkpoint_id,
                created_after,
                created_before,
            )
            .await
    }
    async fn insert_event(
        &self,
        tenant_id: &TenantId,
        event: &EventRecord,
    ) -> Result<(), CoreError> {
        if *self.remove_parent_before_event.lock().unwrap() {
            self.inner
                .delete_instances_batch(tenant_id, std::slice::from_ref(&event.instance_id))
                .await?;
        }
        self.inner.insert_event(tenant_id, event).await
    }
    async fn insert_signal(
        &self,
        tenant_id: &TenantId,
        instance_id: &str,
        signal_type: SignalType,
        payload: &[u8],
    ) -> Result<(), CoreError> {
        self.inner
            .insert_signal(tenant_id, instance_id, signal_type, payload)
            .await
    }
    async fn get_pending_signal(
        &self,
        tenant_id: &TenantId,
        instance_id: &str,
    ) -> Result<Option<SignalRecord>, CoreError> {
        self.fail_if(&self.fail_signal_read, "get_pending_signal")?;
        self.inner.get_pending_signal(tenant_id, instance_id).await
    }
    async fn acknowledge_signal(
        &self,
        tenant_id: &TenantId,
        instance_id: &str,
        command_id: &str,
        signal_type: SignalType,
    ) -> Result<bool, CoreError> {
        self.inner
            .acknowledge_signal(tenant_id, instance_id, command_id, signal_type)
            .await
    }
    async fn apply_lifecycle_command(
        &self,
        tenant_id: &TenantId,
        instance_id: &str,
        command_id: &str,
        signal_type: SignalType,
    ) -> Result<crate::lifecycle::Decision, CoreError> {
        self.fail_if(&self.fail_lifecycle, "apply_lifecycle_command")?;
        self.inner
            .apply_lifecycle_command(tenant_id, instance_id, command_id, signal_type)
            .await
    }
    async fn park_instance(
        &self,
        tenant_id: &TenantId,
        instance_id: &str,
        request: crate::lifecycle::ParkRequest,
    ) -> Result<crate::lifecycle::Decision, CoreError> {
        self.inner
            .park_instance(tenant_id, instance_id, request)
            .await
    }
    async fn cancel_suspended_instances(
        &self,
        tenant_id: &TenantId,
        instance_id: Option<&str>,
        limit: i64,
    ) -> Result<Vec<CancelledInstance>, CoreError> {
        self.inner
            .cancel_suspended_instances(tenant_id, instance_id, limit)
            .await
    }
    async fn put_custom_signal(
        &self,
        tenant_id: &TenantId,
        instance_id: &str,
        checkpoint_id: &str,
        payload: &[u8],
    ) -> Result<String, CoreError> {
        self.inner
            .put_custom_signal(tenant_id, instance_id, checkpoint_id, payload)
            .await
    }
    async fn get_custom_signal(
        &self,
        tenant_id: &TenantId,
        instance_id: &str,
        checkpoint_id: &str,
    ) -> Result<Option<CustomSignalRecord>, CoreError> {
        self.inner
            .get_custom_signal(tenant_id, instance_id, checkpoint_id)
            .await
    }
    async fn save_retry_attempt(
        &self,
        tenant_id: &TenantId,
        instance_id: &str,
        checkpoint_id: &str,
        attempt: i32,
        error_message: Option<&str>,
    ) -> Result<(), CoreError> {
        self.inner
            .save_retry_attempt(
                tenant_id,
                instance_id,
                checkpoint_id,
                attempt,
                error_message,
            )
            .await
    }
    async fn list_instances(
        &self,
        tenant_id: &TenantId,
        status: Option<InstanceStatus>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<InstanceRecord>, CoreError> {
        self.inner
            .list_instances(tenant_id, status, limit, offset)
            .await
    }
    async fn health_check(&self) -> Result<bool, CoreError> {
        self.inner.health_check().await
    }
    async fn count_active_instances(&self, tenant_id: &TenantId) -> Result<i64, CoreError> {
        self.fail_if(&self.fail_count, "count_active_instances")?;
        if let Some(count) = *self.active_instance_count.lock().unwrap() {
            return Ok(count);
        }
        self.inner.count_active_instances(tenant_id).await
    }
    async fn mark_instance_running(
        &self,
        tenant_id: &TenantId,
        instance_id: &str,
        started_at: DateTime<Utc>,
    ) -> Result<(), CoreError> {
        self.inner
            .mark_instance_running(tenant_id, instance_id, started_at)
            .await
    }
    async fn mark_instance_started(
        &self,
        tenant_id: &TenantId,
        instance_id: &str,
        started_at: DateTime<Utc>,
    ) -> Result<bool, CoreError> {
        self.inner
            .mark_instance_started(tenant_id, instance_id, started_at)
            .await
    }
    async fn set_instance_sleep(
        &self,
        tenant_id: &TenantId,
        instance_id: &str,
        sleep_until: DateTime<Utc>,
    ) -> Result<(), CoreError> {
        self.inner
            .set_instance_sleep(tenant_id, instance_id, sleep_until)
            .await
    }
    async fn schedule_wake(
        &self,
        tenant_id: &TenantId,
        instance_id: &str,
        deadline: DateTime<Utc>,
        reason: crate::domain::WakeReason,
    ) -> Result<(), CoreError> {
        self.inner
            .schedule_wake(tenant_id, instance_id, deadline, reason)
            .await
    }
    async fn clear_instance_sleep(
        &self,
        tenant_id: &TenantId,
        instance_id: &str,
    ) -> Result<(), CoreError> {
        self.inner
            .clear_instance_sleep(tenant_id, instance_id)
            .await
    }
    async fn claim_sleeping_instance(
        &self,
        tenant_id: &TenantId,
        instance_id: &str,
    ) -> Result<bool, CoreError> {
        self.inner
            .claim_sleeping_instance(tenant_id, instance_id)
            .await
    }
    async fn get_sleeping_instances_due(
        &self,
        tenant_id: &TenantId,
        limit: i64,
    ) -> Result<Vec<InstanceRecord>, CoreError> {
        self.inner
            .get_sleeping_instances_due(tenant_id, limit)
            .await
    }
    async fn claim_sleeping_instances_due(
        &self,
        tenant_id: &TenantId,
        limit: i64,
        retry_at: DateTime<Utc>,
    ) -> Result<Vec<InstanceRecord>, CoreError> {
        self.inner
            .claim_sleeping_instances_due(tenant_id, limit, retry_at)
            .await
    }
    async fn list_events(
        &self,
        tenant_id: &TenantId,
        instance_id: &str,
        filter: &ListEventsFilter,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<EventRecord>, CoreError> {
        self.inner
            .list_events(tenant_id, instance_id, filter, limit, offset)
            .await
    }
    async fn count_events(
        &self,
        tenant_id: &TenantId,
        instance_id: &str,
        filter: &ListEventsFilter,
    ) -> Result<i64, CoreError> {
        self.inner
            .count_events(tenant_id, instance_id, filter)
            .await
    }
    async fn list_paired_records(
        &self,
        tenant_id: &TenantId,
        instance_id: &str,
        vocabulary: &EventVocabulary,
        filter: &ListPairedRecordsFilter,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<PairedRecordSummary>, CoreError> {
        self.inner
            .list_paired_records(tenant_id, instance_id, vocabulary, filter, limit, offset)
            .await
    }
    async fn count_paired_records(
        &self,
        tenant_id: &TenantId,
        instance_id: &str,
        vocabulary: &EventVocabulary,
        filter: &ListPairedRecordsFilter,
    ) -> Result<i64, CoreError> {
        self.inner
            .count_paired_records(tenant_id, instance_id, vocabulary, filter)
            .await
    }
    async fn get_terminal_instances_older_than(
        &self,
        tenant_id: &TenantId,
        older_than: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<String>, CoreError> {
        self.inner
            .get_terminal_instances_older_than(tenant_id, older_than, limit)
            .await
    }
    async fn delete_instances_batch(
        &self,
        tenant_id: &TenantId,
        instance_ids: &[String],
    ) -> Result<u64, CoreError> {
        self.inner
            .delete_instances_batch(tenant_id, instance_ids)
            .await
    }
    async fn delete_paired_events_older_than(
        &self,
        tenant_id: &TenantId,
        vocabulary: &EventVocabulary,
        older_than: DateTime<Utc>,
        limit: i64,
    ) -> Result<u64, CoreError> {
        self.inner
            .delete_paired_events_older_than(tenant_id, vocabulary, older_than, limit)
            .await
    }
}
