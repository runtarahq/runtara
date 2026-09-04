// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! An in-memory [`Persistence`] backend.
//!
//! Written from the [`Persistence`] trait's own documentation — the contract a
//! backend author actually has — rather than from the SQL backend or the
//! conformance suite. Where the trait says a default implementation is adequate
//! for an in-memory backend, this takes the default rather than overriding it,
//! so any gap between what the docs promise and what the contract requires
//! shows up as a conformance failure instead of being papered over.
//!
//! A single mutex covers the whole store, which is what makes the claim and
//! guard operations atomic without any SQL.

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::error::CoreError;
use crate::persistence::{
    CheckpointRecord, CompleteInstanceGuard, CompleteInstanceParams, CustomSignalRecord,
    EventRecord, InstanceRecord, Persistence, SignalRecord,
};

#[derive(Default)]
struct Store {
    instances: HashMap<String, InstanceRecord>,
    /// Ordered by insertion; `(instance_id, checkpoint_id)` is unique.
    checkpoints: Vec<CheckpointRecord>,
    /// Ordered by insertion, which is also the tiebreak for equal timestamps.
    events: Vec<EventRecord>,
    /// One pending signal per instance.
    signals: HashMap<String, SignalRecord>,
    custom_signals: HashMap<(String, String), CustomSignalRecord>,
    /// Stands in for a sequence: the only monotonic id source a store without
    /// one has to invent.
    next_id: i64,
}

impl Store {
    fn next_id(&mut self) -> i64 {
        self.next_id += 1;
        self.next_id
    }

    fn instance_mut(&mut self, instance_id: &str) -> Result<&mut InstanceRecord, CoreError> {
        self.instances
            .get_mut(instance_id)
            .ok_or_else(|| CoreError::InstanceNotFound {
                instance_id: instance_id.to_string(),
            })
    }
}

/// A `Persistence` implementation backed by process memory.
#[derive(Default)]
pub struct InMemoryPersistence {
    store: Mutex<Store>,
}

impl InMemoryPersistence {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }
}

/// Statuses that stamp `finished_at`.
fn stamps_finished_at(status: &str) -> bool {
    matches!(status, "completed" | "failed" | "cancelled" | "suspended")
}

#[async_trait]
impl Persistence for InMemoryPersistence {
    async fn register_instance(&self, instance_id: &str, tenant_id: &str) -> Result<(), CoreError> {
        let mut store = self.store.lock().unwrap();
        if store.instances.contains_key(instance_id) {
            return Err(CoreError::InstanceAlreadyExists {
                instance_id: instance_id.to_string(),
            });
        }
        store.instances.insert(
            instance_id.to_string(),
            InstanceRecord {
                instance_id: instance_id.to_string(),
                tenant_id: tenant_id.to_string(),
                definition_version: 1,
                status: "pending".to_string(),
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
                recovery_attempts: 0,
                recovery_marker: None,
            },
        );
        Ok(())
    }

    async fn get_instance(&self, instance_id: &str) -> Result<Option<InstanceRecord>, CoreError> {
        Ok(self
            .store
            .lock()
            .unwrap()
            .instances
            .get(instance_id)
            .cloned())
    }

    async fn update_instance_status(
        &self,
        instance_id: &str,
        status: &str,
        started_at: Option<DateTime<Utc>>,
    ) -> Result<(), CoreError> {
        let mut store = self.store.lock().unwrap();
        let inst = store.instance_mut(instance_id)?;
        inst.status = status.to_string();
        if let Some(at) = started_at {
            inst.started_at = Some(at);
        }
        Ok(())
    }

    async fn update_instance_checkpoint(
        &self,
        instance_id: &str,
        checkpoint_id: &str,
    ) -> Result<(), CoreError> {
        let mut store = self.store.lock().unwrap();
        store.instance_mut(instance_id)?.checkpoint_id = Some(checkpoint_id.to_string());
        Ok(())
    }

    async fn complete_instance(
        &self,
        params: CompleteInstanceParams<'_>,
    ) -> Result<bool, CoreError> {
        let mut store = self.store.lock().unwrap();
        let Some(inst) = store.instances.get_mut(params.instance_id) else {
            return match params.guard {
                CompleteInstanceGuard::Any => Err(CoreError::InstanceNotFound {
                    instance_id: params.instance_id.to_string(),
                }),
                CompleteInstanceGuard::OnlyRunning => Ok(false),
            };
        };
        if params.guard == CompleteInstanceGuard::OnlyRunning && inst.status != "running" {
            return Ok(false);
        }

        inst.status = params.status.to_string();
        // Per CompleteInstanceParams: "All optional fields use COALESCE
        // semantics -- None leaves the existing value unchanged."
        if let Some(v) = params.termination_reason {
            inst.termination_reason = Some(v.to_string());
        }
        if let Some(v) = params.exit_code {
            inst.exit_code = Some(v);
        }
        if let Some(v) = params.output {
            inst.output = Some(v.to_vec());
        }
        if let Some(v) = params.error {
            inst.error = Some(v.to_string());
        }
        if let Some(v) = params.checkpoint_id {
            inst.checkpoint_id = Some(v.to_string());
        }
        if stamps_finished_at(params.status) {
            inst.finished_at = Some(Utc::now());
        }
        Ok(true)
    }

    async fn store_instance_input(&self, instance_id: &str, input: &[u8]) -> Result<(), CoreError> {
        let mut store = self.store.lock().unwrap();
        if let Some(inst) = store.instances.get_mut(instance_id) {
            inst.input = Some(input.to_vec());
        }
        Ok(())
    }

    async fn save_checkpoint(
        &self,
        instance_id: &str,
        checkpoint_id: &str,
        state: &[u8],
    ) -> Result<(), CoreError> {
        let mut store = self.store.lock().unwrap();
        if let Some(existing) = store
            .checkpoints
            .iter_mut()
            .find(|c| c.instance_id == instance_id && c.checkpoint_id == checkpoint_id)
        {
            existing.state = state.to_vec();
            return Ok(());
        }
        let id = store.next_id();
        store.checkpoints.push(CheckpointRecord {
            id,
            instance_id: instance_id.to_string(),
            checkpoint_id: checkpoint_id.to_string(),
            state: state.to_vec(),
            created_at: Utc::now(),
        });
        Ok(())
    }

    async fn load_checkpoint(
        &self,
        instance_id: &str,
        checkpoint_id: &str,
    ) -> Result<Option<CheckpointRecord>, CoreError> {
        Ok(self
            .store
            .lock()
            .unwrap()
            .checkpoints
            .iter()
            .find(|c| c.instance_id == instance_id && c.checkpoint_id == checkpoint_id)
            .cloned())
    }

    async fn list_checkpoints(
        &self,
        instance_id: &str,
        checkpoint_id: Option<&str>,
        limit: i64,
        offset: i64,
        created_after: Option<DateTime<Utc>>,
        created_before: Option<DateTime<Utc>>,
    ) -> Result<Vec<CheckpointRecord>, CoreError> {
        let store = self.store.lock().unwrap();
        Ok(store
            .checkpoints
            .iter()
            .filter(|c| c.instance_id == instance_id)
            .filter(|c| checkpoint_id.is_none_or(|id| c.checkpoint_id == id))
            .filter(|c| created_after.is_none_or(|t| c.created_at >= t))
            .filter(|c| created_before.is_none_or(|t| c.created_at < t))
            .skip(offset.max(0) as usize)
            .take(limit.max(0) as usize)
            .cloned()
            .collect())
    }

    async fn count_checkpoints(
        &self,
        instance_id: &str,
        checkpoint_id: Option<&str>,
        created_after: Option<DateTime<Utc>>,
        created_before: Option<DateTime<Utc>>,
    ) -> Result<i64, CoreError> {
        let store = self.store.lock().unwrap();
        Ok(store
            .checkpoints
            .iter()
            .filter(|c| c.instance_id == instance_id)
            .filter(|c| checkpoint_id.is_none_or(|id| c.checkpoint_id == id))
            .filter(|c| created_after.is_none_or(|t| c.created_at >= t))
            .filter(|c| created_before.is_none_or(|t| c.created_at < t))
            .count() as i64)
    }

    async fn insert_event(&self, event: &EventRecord) -> Result<(), CoreError> {
        let mut store = self.store.lock().unwrap();
        let id = store.next_id();
        let mut stored = event.clone();
        // The docs are explicit: store the emitter's `created_at` verbatim.
        stored.id = Some(id);
        store.events.push(stored);
        Ok(())
    }

    async fn insert_signal(
        &self,
        instance_id: &str,
        signal_type: &str,
        payload: &[u8],
    ) -> Result<(), CoreError> {
        let mut store = self.store.lock().unwrap();
        store.signals.insert(
            instance_id.to_string(),
            SignalRecord {
                instance_id: instance_id.to_string(),
                signal_type: signal_type.to_string(),
                payload: (!payload.is_empty()).then(|| payload.to_vec()),
                created_at: Utc::now(),
                acknowledged_at: None,
            },
        );
        Ok(())
    }

    async fn get_pending_signal(
        &self,
        instance_id: &str,
    ) -> Result<Option<SignalRecord>, CoreError> {
        Ok(self
            .store
            .lock()
            .unwrap()
            .signals
            .get(instance_id)
            .filter(|s| s.acknowledged_at.is_none())
            .cloned())
    }

    async fn acknowledge_signal(&self, instance_id: &str) -> Result<(), CoreError> {
        let mut store = self.store.lock().unwrap();
        if let Some(signal) = store.signals.get_mut(instance_id) {
            signal.acknowledged_at = Some(Utc::now());
        }
        Ok(())
    }

    async fn insert_custom_signal(
        &self,
        instance_id: &str,
        checkpoint_id: &str,
        payload: &[u8],
    ) -> Result<(), CoreError> {
        let mut store = self.store.lock().unwrap();
        store.custom_signals.insert(
            (instance_id.to_string(), checkpoint_id.to_string()),
            CustomSignalRecord {
                instance_id: instance_id.to_string(),
                checkpoint_id: checkpoint_id.to_string(),
                payload: (!payload.is_empty()).then(|| payload.to_vec()),
                created_at: Utc::now(),
            },
        );
        Ok(())
    }

    async fn take_pending_custom_signal(
        &self,
        instance_id: &str,
        checkpoint_id: &str,
    ) -> Result<Option<CustomSignalRecord>, CoreError> {
        // Non-destructive: a replayed WaitForSignal must re-read the same row.
        Ok(self
            .store
            .lock()
            .unwrap()
            .custom_signals
            .get(&(instance_id.to_string(), checkpoint_id.to_string()))
            .cloned())
    }

    async fn save_retry_attempt(
        &self,
        instance_id: &str,
        checkpoint_id: &str,
        attempt: i32,
        _error_message: Option<&str>,
    ) -> Result<(), CoreError> {
        // Stored as a synthetic checkpoint, matching the documented shape.
        self.save_checkpoint(
            instance_id,
            &format!("{checkpoint_id}::retry::{attempt}"),
            b"",
        )
        .await
    }

    async fn list_instances(
        &self,
        tenant_id: Option<&str>,
        status: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<InstanceRecord>, CoreError> {
        let store = self.store.lock().unwrap();
        let mut found: Vec<_> = store
            .instances
            .values()
            .filter(|i| tenant_id.is_none_or(|t| i.tenant_id == t))
            .filter(|i| status.is_none_or(|s| i.status == s))
            .cloned()
            .collect();
        // Newest first, matching the SQL backend's ORDER BY created_at DESC.
        found.sort_by_key(|i| std::cmp::Reverse(i.created_at));
        Ok(found
            .into_iter()
            .skip(offset.max(0) as usize)
            .take(limit.max(0) as usize)
            .collect())
    }

    async fn health_check_db(&self) -> Result<bool, CoreError> {
        Ok(true)
    }

    async fn count_active_instances(&self) -> Result<i64, CoreError> {
        Ok(self
            .store
            .lock()
            .unwrap()
            .instances
            .values()
            .filter(|i| i.status == "running")
            .count() as i64)
    }

    async fn set_instance_sleep(
        &self,
        instance_id: &str,
        sleep_until: DateTime<Utc>,
    ) -> Result<(), CoreError> {
        let mut store = self.store.lock().unwrap();
        store.instance_mut(instance_id)?.sleep_until = Some(sleep_until);
        Ok(())
    }

    async fn clear_instance_sleep(&self, instance_id: &str) -> Result<(), CoreError> {
        let mut store = self.store.lock().unwrap();
        store.instance_mut(instance_id)?.sleep_until = None;
        Ok(())
    }

    /// Claimed under the store lock, so two concurrent callers cannot both
    /// win -- the default's read-then-clear can.
    async fn claim_sleeping_instance(&self, instance_id: &str) -> Result<bool, CoreError> {
        let mut store = self.store.lock().unwrap();
        let Some(instance) = store.instances.get_mut(instance_id) else {
            return Ok(false);
        };
        // Due-ness, not just presence: an instance leased by a batch claim has
        // `sleep_until` in the future and must lose until the lease expires.
        if instance.status != "suspended" || !instance.sleep_until.is_some_and(|t| t <= Utc::now())
        {
            return Ok(false);
        }
        instance.sleep_until = None;
        Ok(true)
    }

    async fn get_sleeping_instances_due(
        &self,
        limit: i64,
    ) -> Result<Vec<InstanceRecord>, CoreError> {
        let now = Utc::now();
        let store = self.store.lock().unwrap();
        let mut due: Vec<_> = store
            .instances
            .values()
            .filter(|i| i.status == "suspended")
            .filter(|i| i.sleep_until.is_some_and(|t| t <= now))
            .cloned()
            .collect();
        due.sort_by_key(|i| i.sleep_until);
        Ok(due.into_iter().take(limit.max(0) as usize).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The probe: the backend contract, run against a store with no SQL in it.
    #[tokio::test]
    async fn in_memory_backend_satisfies_the_conformance_sequence() {
        let backend = InMemoryPersistence::new();
        crate::persistence::common::ops::postgres_conformance::run_conformance_sequence(&backend)
            .await;
    }
}
