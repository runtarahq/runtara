// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! In-memory `Persistence` mock used by the handler unit tests.
//!
//! This module is compiled only under `#[cfg(test)]`, so it carries zero cost
//! in release builds. Each handler submodule's `mod tests` imports the mock
//! via `crate::instance_handlers::mock_persistence::*`.

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::error::CoreError;
use crate::persistence::{
    CheckpointRecord, CustomSignalRecord, EventRecord, InstanceRecord, ListEventsFilter,
    ListStepSummariesFilter, Persistence, SignalRecord, StepSummaryRecord,
};

/// Mock persistence for handler unit tests.
pub(crate) struct MockPersistence {
    instances: Mutex<HashMap<String, InstanceRecord>>,
    checkpoints: Mutex<HashMap<(String, String), CheckpointRecord>>,
    signals: Mutex<HashMap<String, SignalRecord>>,
    events: Mutex<Vec<EventRecord>>,
    custom_signals: Mutex<HashMap<(String, String), CustomSignalRecord>>,
    fail_register: Mutex<bool>,
    fail_status_update: Mutex<bool>,
    active_instance_count: Mutex<Option<i64>>,
}

impl MockPersistence {
    pub(crate) fn new() -> Self {
        Self {
            instances: Mutex::new(HashMap::new()),
            checkpoints: Mutex::new(HashMap::new()),
            signals: Mutex::new(HashMap::new()),
            events: Mutex::new(Vec::new()),
            custom_signals: Mutex::new(HashMap::new()),
            fail_register: Mutex::new(false),
            fail_status_update: Mutex::new(false),
            active_instance_count: Mutex::new(None),
        }
    }

    /// Override the value returned by `count_active_instances` (default: 0).
    pub(crate) fn with_active_count(self, count: i64) -> Self {
        *self.active_instance_count.lock().unwrap() = Some(count);
        self
    }

    pub(crate) fn with_instance(self, instance: InstanceRecord) -> Self {
        self.instances
            .lock()
            .unwrap()
            .insert(instance.instance_id.clone(), instance);
        self
    }

    pub(crate) fn with_checkpoint(self, checkpoint: CheckpointRecord) -> Self {
        self.checkpoints.lock().unwrap().insert(
            (
                checkpoint.instance_id.clone(),
                checkpoint.checkpoint_id.clone(),
            ),
            checkpoint,
        );
        self
    }

    pub(crate) fn with_signal(self, signal: SignalRecord) -> Self {
        self.signals
            .lock()
            .unwrap()
            .insert(signal.instance_id.clone(), signal);
        self
    }

    pub(crate) fn with_custom_signal(self, signal: CustomSignalRecord) -> Self {
        self.custom_signals.lock().unwrap().insert(
            (signal.instance_id.clone(), signal.checkpoint_id.clone()),
            signal,
        );
        self
    }

    #[allow(dead_code)]
    pub(crate) fn set_fail_register(&self) {
        *self.fail_register.lock().unwrap() = true;
    }

    #[allow(dead_code)]
    pub(crate) fn set_fail_status_update(&self) {
        *self.fail_status_update.lock().unwrap() = true;
    }

    pub(crate) fn get_events(&self) -> Vec<EventRecord> {
        self.events.lock().unwrap().clone()
    }
}

pub(crate) fn make_instance(instance_id: &str, tenant_id: &str, status: &str) -> InstanceRecord {
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
        input: None,
        output: None,
        error: None,
        sleep_until: None,
        termination_reason: None,
        exit_code: None,
    }
}

pub(crate) fn make_checkpoint(
    instance_id: &str,
    checkpoint_id: &str,
    state: &[u8],
) -> CheckpointRecord {
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

pub(crate) fn make_signal(instance_id: &str, signal_type: &str) -> SignalRecord {
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

    async fn complete_instance_if_running(
        &self,
        instance_id: &str,
        status: &str,
        output: Option<&[u8]>,
        error: Option<&str>,
        _stderr: Option<&str>,
        checkpoint_id: Option<&str>,
    ) -> std::result::Result<bool, CoreError> {
        let mut instances = self.instances.lock().unwrap();
        if let Some(inst) = instances.get_mut(instance_id)
            && inst.status == "running"
        {
            inst.status = status.to_string();
            inst.output = output.map(|o| o.to_vec());
            inst.error = error.map(|e| e.to_string());
            inst.checkpoint_id = checkpoint_id.map(|s| s.to_string());
            if status == "completed"
                || status == "failed"
                || status == "cancelled"
                || status == "suspended"
            {
                inst.finished_at = Some(Utc::now());
            }
            return Ok(true);
        }
        Ok(false)
    }

    async fn complete_instance_with_termination(
        &self,
        instance_id: &str,
        status: &str,
        termination_reason: Option<&str>,
        exit_code: Option<i32>,
        output: Option<&[u8]>,
        error: Option<&str>,
        _stderr: Option<&str>,
        checkpoint_id: Option<&str>,
    ) -> std::result::Result<(), CoreError> {
        if let Some(inst) = self.instances.lock().unwrap().get_mut(instance_id) {
            inst.status = status.to_string();
            inst.termination_reason = termination_reason.map(|s| s.to_string());
            inst.exit_code = exit_code;
            inst.output = output.map(|o| o.to_vec());
            inst.error = error.map(|e| e.to_string());
            inst.checkpoint_id = checkpoint_id.map(|s| s.to_string());
            if status == "completed" || status == "failed" || status == "cancelled" {
                inst.finished_at = Some(Utc::now());
            }
        }
        Ok(())
    }

    async fn complete_instance_with_termination_if_running(
        &self,
        instance_id: &str,
        status: &str,
        termination_reason: Option<&str>,
        exit_code: Option<i32>,
        output: Option<&[u8]>,
        error: Option<&str>,
        _stderr: Option<&str>,
        checkpoint_id: Option<&str>,
    ) -> std::result::Result<bool, CoreError> {
        let mut instances = self.instances.lock().unwrap();
        if let Some(inst) = instances.get_mut(instance_id)
            && inst.status == "running"
        {
            inst.status = status.to_string();
            inst.termination_reason = termination_reason.map(|s| s.to_string());
            inst.exit_code = exit_code;
            inst.output = output.map(|o| o.to_vec());
            inst.error = error.map(|e| e.to_string());
            inst.checkpoint_id = checkpoint_id.map(|s| s.to_string());
            if status == "completed" || status == "failed" || status == "cancelled" {
                inst.finished_at = Some(Utc::now());
            }
            return Ok(true);
        }
        Ok(false)
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

    async fn acknowledge_signal(&self, instance_id: &str) -> std::result::Result<(), CoreError> {
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
        Ok(self.active_instance_count.lock().unwrap().unwrap_or(0))
    }

    async fn set_instance_sleep(
        &self,
        instance_id: &str,
        sleep_until: DateTime<Utc>,
    ) -> std::result::Result<(), CoreError> {
        if let Some(inst) = self.instances.lock().unwrap().get_mut(instance_id) {
            inst.sleep_until = Some(sleep_until);
        }
        Ok(())
    }

    async fn clear_instance_sleep(&self, _instance_id: &str) -> std::result::Result<(), CoreError> {
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
