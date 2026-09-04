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
    EventRecord, EventSortOrder, EventVocabulary, InstanceRecord, ListEventsFilter,
    ListPairedRecordsFilter, PairedRecordStatus, PairedRecordSummary, Persistence, SignalRecord,
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

    async fn list_events(
        &self,
        instance_id: &str,
        filter: &ListEventsFilter,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<EventRecord>, CoreError> {
        let store = self.store.lock().unwrap();
        let mut found = filtered_events(&store, instance_id, filter);
        // Ordered by `(created_at, id)`: the id breaks ties so events written
        // inside one clock tick still read back in insertion order.
        found.sort_by_key(|e| (e.created_at, e.id));
        if filter.sort_order == EventSortOrder::Desc {
            found.reverse();
        }
        Ok(found
            .into_iter()
            .skip(offset.max(0) as usize)
            .take(limit.max(0) as usize)
            .cloned()
            .collect())
    }

    async fn count_events(
        &self,
        instance_id: &str,
        filter: &ListEventsFilter,
    ) -> Result<i64, CoreError> {
        let store = self.store.lock().unwrap();
        Ok(filtered_events(&store, instance_id, filter).len() as i64)
    }

    async fn list_paired_records(
        &self,
        instance_id: &str,
        vocabulary: &EventVocabulary,
        filter: &ListPairedRecordsFilter,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<PairedRecordSummary>, CoreError> {
        let store = self.store.lock().unwrap();
        let mut paired = pair_records(&store, instance_id, vocabulary);
        paired.retain(|record| matches_filter(record, filter));
        paired.sort_by_key(|r| r.started_at);
        if filter.sort_order == EventSortOrder::Desc {
            paired.reverse();
        }
        Ok(paired
            .into_iter()
            .skip(offset.max(0) as usize)
            .take(limit.max(0) as usize)
            .collect())
    }

    async fn count_paired_records(
        &self,
        instance_id: &str,
        vocabulary: &EventVocabulary,
        filter: &ListPairedRecordsFilter,
    ) -> Result<i64, CoreError> {
        let store = self.store.lock().unwrap();
        Ok(pair_records(&store, instance_id, vocabulary)
            .into_iter()
            .filter(|record| matches_filter(record, filter))
            .count() as i64)
    }

    /// Terminal instances that finished before `older_than`, oldest first.
    ///
    /// The trait's default returns an empty list, which reads as "nothing to
    /// sweep" rather than "not implemented" -- a backend that takes it loses
    /// retention silently, so this implements it.
    async fn get_terminal_instances_older_than(
        &self,
        older_than: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<String>, CoreError> {
        let store = self.store.lock().unwrap();
        let mut terminal: Vec<_> = store
            .instances
            .values()
            .filter(|i| matches!(i.status.as_str(), "completed" | "failed" | "cancelled"))
            .filter(|i| i.finished_at.is_some_and(|t| t < older_than))
            .collect();
        terminal.sort_by_key(|i| i.finished_at);
        Ok(terminal
            .into_iter()
            .take(limit.max(0) as usize)
            .map(|i| i.instance_id.clone())
            .collect())
    }

    /// Delete instances and everything hanging off them.
    ///
    /// The SQL backend gets the cascade from the schema; here the child
    /// collections are pruned explicitly, which is the same guarantee written
    /// out by hand.
    async fn delete_instances_batch(&self, instance_ids: &[String]) -> Result<u64, CoreError> {
        let mut store = self.store.lock().unwrap();
        let mut deleted = 0;
        for id in instance_ids {
            if store.instances.remove(id).is_some() {
                deleted += 1;
            }
            store.checkpoints.retain(|c| &c.instance_id != id);
            store.events.retain(|e| &e.instance_id != id);
            store.signals.remove(id);
            store.custom_signals.retain(|(inst, _), _| inst != id);
        }
        Ok(deleted)
    }

    /// Delete only the paired events the vocabulary names, oldest first.
    ///
    /// Lifecycle events are the run's history and go only when the instance
    /// does; a subtype this vocabulary does not name is left alone even if it
    /// is older than the cutoff.
    async fn delete_paired_events_older_than(
        &self,
        vocabulary: &EventVocabulary,
        older_than: DateTime<Utc>,
        limit: i64,
    ) -> Result<u64, CoreError> {
        let mut store = self.store.lock().unwrap();
        let paired = [vocabulary.start_subtype(), vocabulary.end_subtype()];
        let mut doomed: Vec<i64> = store
            .events
            .iter()
            .filter(|e| e.created_at < older_than)
            .filter(|e| e.subtype.as_deref().is_some_and(|s| paired.contains(&s)))
            .filter_map(|e| e.id)
            .collect();
        doomed.sort_unstable();
        doomed.truncate(limit.max(0) as usize);
        let before = store.events.len();
        store
            .events
            .retain(|e| !e.id.is_some_and(|id| doomed.contains(&id)));
        Ok((before - store.events.len()) as u64)
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

/// Events for one instance that satisfy `filter`.
///
/// `payload_contains` is a case-insensitive substring match over the decoded
/// bytes, and the scope predicates read two keys out of the payload — the same
/// three the SQL backend expresses as `ILIKE` and JSON path lookups.
fn filtered_events<'s>(
    store: &'s Store,
    instance_id: &str,
    filter: &ListEventsFilter,
) -> Vec<&'s EventRecord> {
    store
        .events
        .iter()
        .filter(|e| e.instance_id == instance_id)
        .filter(|e| {
            filter
                .event_type
                .as_ref()
                .is_none_or(|t| &e.event_type == t)
        })
        .filter(|e| {
            filter
                .subtype
                .as_deref()
                .is_none_or(|s| e.subtype.as_deref() == Some(s))
        })
        .filter(|e| filter.created_after.is_none_or(|t| e.created_at >= t))
        .filter(|e| filter.created_before.is_none_or(|t| e.created_at < t))
        .filter(|e| {
            filter.payload_contains.as_ref().is_none_or(|needle| {
                payload_text(e)
                    .is_some_and(|text| text.to_lowercase().contains(&needle.to_lowercase()))
            })
        })
        .filter(|e| {
            filter
                .scope_id
                .as_deref()
                .is_none_or(|want| payload_str(e, "scope_id").as_deref() == Some(want))
        })
        .filter(|e| {
            filter
                .parent_scope_id
                .as_deref()
                .is_none_or(|want| payload_str(e, "parent_scope_id").as_deref() == Some(want))
        })
        .filter(|e| !filter.root_scopes_only || payload_str(e, "parent_scope_id").is_none())
        .collect()
}

fn payload_text(event: &EventRecord) -> Option<String> {
    let bytes = event.payload.as_ref()?;
    String::from_utf8(bytes.clone()).ok()
}

fn payload_json(event: &EventRecord) -> Option<serde_json::Value> {
    serde_json::from_str(&payload_text(event)?).ok()
}

fn payload_str(event: &EventRecord, key: &str) -> Option<String> {
    payload_json(event)?.get(key)?.as_str().map(str::to_string)
}

/// Pair each start event with its end event, using only the names the caller
/// supplied.
///
/// This function is the reason the in-memory backend is worth having: it
/// implements the pairing rule with no SQL at all, so a vocabulary key that the
/// kernel secretly special-cased would show up here as a name this code had to
/// know. It knows none of them — every key comes from `vocabulary`.
///
/// A start and an end are the same record when their correlation values match
/// *and* their scopes match, so the same step id inside two different loop
/// iterations stays two records.
fn pair_records(
    store: &Store,
    instance_id: &str,
    vocabulary: &EventVocabulary,
) -> Vec<PairedRecordSummary> {
    let of_subtype = |subtype: &str| -> Vec<&EventRecord> {
        store
            .events
            .iter()
            .filter(|e| e.instance_id == instance_id)
            .filter(|e| e.subtype.as_deref() == Some(subtype))
            .collect()
    };
    let ends = of_subtype(vocabulary.end_subtype());

    let mut records = Vec::new();
    for start in of_subtype(vocabulary.start_subtype()) {
        let correlation_id = payload_str(start, vocabulary.correlation_key()).unwrap_or_default();
        let scope_id = payload_str(start, "scope_id");

        let end = ends.iter().find(|e| {
            payload_str(e, vocabulary.correlation_key()).unwrap_or_default() == correlation_id
                && payload_str(e, "scope_id") == scope_id
        });

        let outputs = end
            .and_then(|e| payload_json(e).and_then(|j| j.get(vocabulary.outputs_key()).cloned()));
        let error = end.and_then(|e| {
            payload_json(e)
                .and_then(|j| j.get(vocabulary.error_key()).cloned())
                .filter(|v| !v.is_null())
        });
        // A producer may report failure by flagging its output rather than by
        // populating the error key.
        let output_failed = outputs
            .as_ref()
            .and_then(|o| o.get(vocabulary.error_flag_key()))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);

        let status = match end {
            None => PairedRecordStatus::Running,
            Some(_) if error.is_some() || output_failed => PairedRecordStatus::Failed,
            Some(_) => PairedRecordStatus::Completed,
        };

        let completed_at = end.map(|e| e.created_at);
        let epoch_ms =
            |e: &EventRecord, key: &str| -> Option<i64> { payload_json(e)?.get(key)?.as_i64() };

        records.push(PairedRecordSummary {
            correlation_id,
            label: payload_str(start, vocabulary.label_key()),
            kind: payload_str(start, vocabulary.kind_key()).unwrap_or_default(),
            status,
            started_at: start.created_at,
            completed_at,
            duration_ms: completed_at.map(|done| (done - start.created_at).num_milliseconds()),
            launched_at_ms: end.and_then(|e| epoch_ms(e, vocabulary.launched_at_key())),
            settled_at_ms: end.and_then(|e| epoch_ms(e, vocabulary.settled_at_key())),
            inputs: payload_json(start).and_then(|j| j.get(vocabulary.inputs_key()).cloned()),
            outputs,
            error,
            scope_id,
            parent_scope_id: payload_str(start, "parent_scope_id"),
        });
    }
    records
}

fn matches_filter(record: &PairedRecordSummary, filter: &ListPairedRecordsFilter) -> bool {
    filter.status.is_none_or(|s| s == record.status)
        && filter.kind.as_ref().is_none_or(|k| &record.kind == k)
        && filter
            .scope_id
            .as_deref()
            .is_none_or(|s| record.scope_id.as_deref() == Some(s))
        && filter
            .parent_scope_id
            .as_deref()
            .is_none_or(|s| record.parent_scope_id.as_deref() == Some(s))
        && (!filter.root_scopes_only || record.parent_scope_id.is_none())
        && filter
            .correlation_ids
            .as_ref()
            .is_none_or(|ids| ids.contains(&record.correlation_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    /// The probe: the backend contract, run against a store with no SQL in it.
    #[tokio::test]
    async fn in_memory_backend_satisfies_the_conformance_sequence() {
        let backend = InMemoryPersistence::new();
        crate::persistence::conformance::run_conformance_sequence(&backend).await;
    }

    fn foreign_vocabulary() -> EventVocabulary {
        // Shares nothing with the workflow DSL: if any name were baked into the
        // pairing, none of these records would pair.
        EventVocabulary::new(crate::persistence::EventVocabularySpec {
            start_subtype: "unit_open",
            end_subtype: "unit_close",
            correlation_key: "unit_id",
            kind_key: "flavour",
            label_key: "caption",
            inputs_key: "given",
            outputs_key: "produced",
            error_key: "trouble",
            error_flag_key: "_broke",
            launched_at_key: "began_ms",
            settled_at_key: "ended_ms",
        })
        .expect("a vocabulary of identifiers is valid")
    }

    async fn append(backend: &InMemoryPersistence, subtype: &str, payload: serde_json::Value) {
        backend
            .insert_event(&EventRecord {
                id: None,
                instance_id: "inst".to_string(),
                event_type: "custom".to_string(),
                checkpoint_id: None,
                payload: Some(payload.to_string().into_bytes()),
                created_at: Utc::now(),
                subtype: Some(subtype.to_string()),
            })
            .await
            .expect("append");
    }

    /// Pairing must be driven entirely by the supplied names.
    #[tokio::test]
    async fn pairs_records_under_a_vocabulary_that_shares_nothing_with_the_dsl() {
        let backend = InMemoryPersistence::new();
        backend.register_instance("inst", "t").await.unwrap();
        let vocabulary = foreign_vocabulary();

        append(
            &backend,
            "unit_open",
            serde_json::json!({"unit_id": "u1", "flavour": "Agent", "caption": "first",
                               "given": {"a": 1}, "scope_id": "s1"}),
        )
        .await;
        append(
            &backend,
            "unit_close",
            serde_json::json!({"unit_id": "u1", "produced": {"ok": true},
                               "scope_id": "s1", "began_ms": 10, "ended_ms": 20}),
        )
        .await;
        append(
            &backend,
            "unit_open",
            serde_json::json!({"unit_id": "u2", "flavour": "Split"}),
        )
        .await;

        let filter = ListPairedRecordsFilter::default();
        let records = backend
            .list_paired_records("inst", &vocabulary, &filter, 50, 0)
            .await
            .expect("list");
        assert_eq!(records.len(), 2);

        let closed = records
            .iter()
            .find(|r| r.correlation_id == "u1")
            .expect("u1 must pair");
        assert_eq!(closed.status, PairedRecordStatus::Completed);
        assert_eq!(closed.kind, "Agent");
        assert_eq!(closed.label.as_deref(), Some("first"));
        assert_eq!(closed.inputs, Some(serde_json::json!({"a": 1})));
        assert_eq!(closed.outputs, Some(serde_json::json!({"ok": true})));
        assert_eq!(closed.launched_at_ms, Some(10));
        assert_eq!(closed.settled_at_ms, Some(20));

        let open = records
            .iter()
            .find(|r| r.correlation_id == "u2")
            .expect("u2 must appear");
        assert_eq!(open.status, PairedRecordStatus::Running);
        assert_eq!(open.completed_at, None);

        assert_eq!(
            backend
                .count_paired_records("inst", &vocabulary, &filter)
                .await
                .unwrap(),
            2
        );
    }

    /// A producer that reports failure by flagging its output, rather than by
    /// populating the error key, still reads as failed.
    #[tokio::test]
    async fn output_flag_marks_a_record_failed() {
        let backend = InMemoryPersistence::new();
        backend.register_instance("inst", "t").await.unwrap();
        let vocabulary = foreign_vocabulary();

        append(&backend, "unit_open", serde_json::json!({"unit_id": "u1"})).await;
        append(
            &backend,
            "unit_close",
            serde_json::json!({"unit_id": "u1", "produced": {"_broke": true}}),
        )
        .await;

        let records = backend
            .list_paired_records(
                "inst",
                &vocabulary,
                &ListPairedRecordsFilter::default(),
                50,
                0,
            )
            .await
            .unwrap();
        assert_eq!(records[0].status, PairedRecordStatus::Failed);
    }

    /// The same correlation id in two scopes is two records, not one.
    #[tokio::test]
    async fn scope_separates_records_sharing_a_correlation_id() {
        let backend = InMemoryPersistence::new();
        backend.register_instance("inst", "t").await.unwrap();
        let vocabulary = foreign_vocabulary();

        for scope in ["iter-1", "iter-2"] {
            append(
                &backend,
                "unit_open",
                serde_json::json!({"unit_id": "same", "scope_id": scope}),
            )
            .await;
        }
        append(
            &backend,
            "unit_close",
            serde_json::json!({"unit_id": "same", "scope_id": "iter-1"}),
        )
        .await;

        let records = backend
            .list_paired_records(
                "inst",
                &vocabulary,
                &ListPairedRecordsFilter::default(),
                50,
                0,
            )
            .await
            .unwrap();
        assert_eq!(records.len(), 2, "one record per scope");
        let by_scope = |s: &str| {
            records
                .iter()
                .find(|r| r.scope_id.as_deref() == Some(s))
                .unwrap()
                .status
        };
        assert_eq!(by_scope("iter-1"), PairedRecordStatus::Completed);
        assert_eq!(
            by_scope("iter-2"),
            PairedRecordStatus::Running,
            "an unclosed iteration must not borrow the other's end event"
        );
    }

    /// Retention removes only what the vocabulary names.
    #[tokio::test]
    async fn the_sweep_spares_subtypes_the_vocabulary_does_not_name() {
        let backend = InMemoryPersistence::new();
        backend.register_instance("inst", "t").await.unwrap();
        let vocabulary = foreign_vocabulary();

        append(&backend, "unit_open", serde_json::json!({"unit_id": "u1"})).await;
        append(&backend, "unit_close", serde_json::json!({"unit_id": "u1"})).await;
        append(&backend, "workflow_log", serde_json::json!({})).await;
        append(&backend, "step_debug_start", serde_json::json!({})).await;

        let deleted = backend
            .delete_paired_events_older_than(&vocabulary, Utc::now() + Duration::seconds(60), 100)
            .await
            .unwrap();
        assert_eq!(deleted, 2, "only the two subtypes this vocabulary names");

        let left = backend
            .list_events("inst", &ListEventsFilter::default(), 50, 0)
            .await
            .unwrap();
        let subtypes: Vec<_> = left.iter().filter_map(|e| e.subtype.as_deref()).collect();
        assert!(subtypes.contains(&"workflow_log"));
        assert!(
            subtypes.contains(&"step_debug_start"),
            "a foreign vocabulary must not sweep the DSL's own subtypes"
        );
    }
}
