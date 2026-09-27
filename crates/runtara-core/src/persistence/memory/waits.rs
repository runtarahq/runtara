//! Instance waits under the store lock. Every writer that finishes a run (or
//! publishes a never-launched outcome) calls [`target_finished`], which does
//! what the Postgres trigger does: stamp a parked waiter whose wait now holds,
//! else leave a nudge for the reconciler.
use super::*;
use crate::persistence::waits::*;

/// One stored wait.
pub(super) struct MemWait {
    pub record: WaitRecord,
    /// Targets whose finish left a nudge the reconciler has not taken yet.
    pub nudged: std::collections::BTreeSet<String>,
    /// Rotation cursor of the reconciler's full pass.
    pub last_reconciled_at: Option<DateTime<Utc>>,
}

fn target_state(store: &Store, tenant: &str, id: &str) -> TargetState {
    if let Some(instance) = store.instances.get(id).filter(|i| i.tenant_id == tenant) {
        return TargetState::Instance {
            status: instance.status,
            finished_at: instance.finished_at,
        };
    }
    match store
        .external_outcomes
        .get(id)
        .filter(|record| record.outcome.tenant_id == tenant)
    {
        Some(record) => TargetState::Outcome {
            outcome: record.outcome.outcome,
            published_at: record.published_at,
        },
        None => TargetState::Unknown,
    }
}

fn states(store: &Store, record: &WaitRecord) -> Vec<WaitTarget> {
    record
        .targets
        .iter()
        .map(|id| WaitTarget {
            instance_id: id.clone(),
            state: target_state(store, &record.tenant_id, id),
        })
        .collect()
}

type Key = (String, String);

/// Resolve a pending wait if the rule says so; the first resolution stands.
fn settle(store: &mut Store, key: &Key, now: DateTime<Utc>) -> Vec<WaitTarget> {
    let record = store.instance_waits[key].record.clone();
    let states = states(store, &record);
    if let Some((resolution, finished)) = record.evaluate(&states, now) {
        let wait = store.instance_waits.get_mut(key).unwrap();
        wait.record.state = WaitState::Resolved {
            resolution,
            resolved_at: now,
            finished,
        };
        wait.nudged.clear();
    }
    states
}

/// Whether `waiter` is parked on `wait_id` and has no wake scheduled yet.
fn parked_on(store: &Store, waiter: &str, wait_id: &str) -> bool {
    store.instances.get(waiter).is_some_and(|instance| {
        instance.status == CoreInstanceStatus::Suspended
            && instance.termination_reason.as_deref() == Some("waiting_instances")
    }) && store
        .input_parks
        .get(waiter)
        .is_some_and(|park| !park.wake_scheduled && park.waits.iter().any(|w| w == wait_id))
}

/// Schedule the parked waiter's wake, once per park.
fn stamp(store: &mut Store, waiter: &str, wait_id: &str, now: DateTime<Utc>) -> bool {
    if !parked_on(store, waiter, wait_id) {
        return false;
    }
    store.input_parks.get_mut(waiter).unwrap().wake_scheduled = true;
    let root = store.instances.get_mut(waiter).unwrap();
    root.sleep_until = Some(root.sleep_until.map_or(now, |deadline| deadline.min(now)));
    root.wake_reason = Some(crate::domain::WakeReason::InstancesTerminal);
    true
}

/// `target` just finished: stamp every parked waiter whose pending wait on
/// it now holds, and nudge the others (the trigger's twin).
pub(super) fn target_finished(store: &mut Store, target: &str, now: DateTime<Utc>) {
    let keys: Vec<Key> = store
        .instance_waits
        .iter()
        .filter(|(_, wait)| {
            wait.record.state == WaitState::Pending
                && wait.record.targets.iter().any(|t| t == target)
        })
        .map(|(key, _)| key.clone())
        .collect();
    for key in keys {
        let record = &store.instance_waits[&key].record;
        let holds = matches!(
            resolve(record.mode, &states(store, record), None, now),
            Some(WaitResolution::Satisfied)
        );
        if !(holds && stamp(store, &key.0, &key.1, now)) {
            store
                .instance_waits
                .get_mut(&key)
                .unwrap()
                .nudged
                .insert(target.to_owned());
        }
    }
}

/// Wake a run that parks on `wait_ids` if any of them is already resolved,
/// closed or missing. Under the park's own lock.
pub(super) fn park_on(store: &mut Store, waiter: &str, wait_ids: &[String], now: DateTime<Utc>) {
    let mut ids = wait_ids.to_vec();
    ids.sort();
    ids.dedup();
    for wait_id in ids {
        let key = (waiter.to_owned(), wait_id.clone());
        let wake = match store.instance_waits.get(&key).map(|w| &w.record.state) {
            Some(WaitState::Pending) => {
                settle(store, &key, now);
                store.instance_waits[&key].record.state != WaitState::Pending
            }
            _ => true,
        };
        if wake {
            stamp(store, waiter, &wait_id, now);
        }
    }
}

fn waiter<'a>(store: &'a Store, tenant: &str, waiter: &str) -> WaitResult<&'a InstanceRecord> {
    store
        .instances
        .get(waiter)
        .filter(|i| i.tenant_id == tenant)
        .ok_or(WaitError::NotFound)
}

fn view(store: &mut Store, key: &Key, now: DateTime<Utc>) -> WaitResult<WaitView> {
    let states = settle(store, key, now);
    WaitView::assemble(store.instance_waits[key].record.clone(), states)
}

#[async_trait]
impl InstanceWaits for InMemoryPersistence {
    async fn register_or_evaluate(
        &self,
        tenant: &str,
        waiter_id: &str,
        wait_id: &str,
        spec: &WaitSpec,
    ) -> WaitResult<WaitView> {
        spec.validate(waiter_id, wait_id)?;
        let mut store = self.store.lock().unwrap();
        if waiter(&store, tenant, waiter_id)?.status.is_terminal() {
            return Err(WaitError::Inactive);
        }
        let key = (waiter_id.to_owned(), wait_id.to_owned());
        let now = Utc::now();
        match store.instance_waits.get(&key).map(|w| &w.record) {
            Some(record) if !matches!(record.state, WaitState::Closed { .. }) => {
                if record.fingerprint != spec.fingerprint() {
                    return Err(WaitError::Conflict);
                }
            }
            _ => {
                store.instance_waits.insert(
                    key.clone(),
                    MemWait {
                        record: WaitRecord {
                            waiter_instance_id: waiter_id.into(),
                            wait_id: wait_id.into(),
                            tenant_id: tenant.into(),
                            mode: spec.mode(),
                            targets: spec.targets().to_vec(),
                            fingerprint: spec.fingerprint(),
                            deadline: spec.deadline(),
                            created_at: now,
                            state: WaitState::Pending,
                        },
                        nudged: Default::default(),
                        last_reconciled_at: None,
                    },
                );
            }
        }
        view(&mut store, &key, now)
    }

    async fn poll_wait(
        &self,
        tenant: &str,
        waiter_id: &str,
        wait_id: &str,
    ) -> WaitResult<WaitView> {
        let mut store = self.store.lock().unwrap();
        waiter(&store, tenant, waiter_id)?;
        let key = (waiter_id.to_owned(), wait_id.to_owned());
        match store.instance_waits.get(&key).map(|w| &w.record.state) {
            None => Err(WaitError::NotFound),
            Some(WaitState::Closed { .. }) => Err(WaitError::Closed),
            Some(_) => view(&mut store, &key, Utc::now()),
        }
    }

    async fn close_wait(&self, tenant: &str, waiter_id: &str, wait_id: &str) -> WaitResult<bool> {
        let mut store = self.store.lock().unwrap();
        waiter(&store, tenant, waiter_id)?;
        let key = (waiter_id.to_owned(), wait_id.to_owned());
        match store.instance_waits.get_mut(&key) {
            Some(wait) if !matches!(wait.record.state, WaitState::Closed { .. }) => {
                wait.record.state = WaitState::Closed {
                    closed_at: Utc::now(),
                };
                wait.nudged.clear();
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    async fn delete_resolved_wait(
        &self,
        tenant: &str,
        waiter_id: &str,
        wait_id: &str,
    ) -> WaitResult<bool> {
        let mut store = self.store.lock().unwrap();
        waiter(&store, tenant, waiter_id)?;
        let key = (waiter_id.to_owned(), wait_id.to_owned());
        match store.instance_waits.get(&key).map(|w| &w.record.state) {
            Some(WaitState::Resolved { .. } | WaitState::Closed { .. }) => {
                store.instance_waits.remove(&key);
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    async fn reconcile_wait_wakes(&self, limit: u32) -> WaitResult<u64> {
        let full = self
            .wait_polls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            .is_multiple_of(FULL_RECONCILE_EVERY);
        let mut store = self.store.lock().unwrap();
        let now = Utc::now();
        let mut woken = std::collections::BTreeSet::new();
        // Pass A: the nudges finishing targets left.
        let mut waiters: Vec<String> = store
            .instance_waits
            .iter()
            .filter(|(_, wait)| !wait.nudged.is_empty())
            .map(|((waiter, _), _)| waiter.clone())
            .collect();
        waiters.sort();
        waiters.dedup();
        waiters.truncate(limit as usize);
        for waiter_id in waiters {
            let keys: Vec<Key> = store
                .instance_waits
                .iter()
                .filter(|((w, _), wait)| *w == waiter_id && !wait.nudged.is_empty())
                .map(|(key, _)| key.clone())
                .collect();
            for key in keys {
                store.instance_waits.get_mut(&key).unwrap().nudged.clear();
                settle(&mut store, &key, now);
                if store.instance_waits[&key].record.state != WaitState::Pending
                    && stamp(&mut store, &key.0, &key.1, now)
                {
                    woken.insert(key.0.clone());
                }
            }
        }
        // Pass B: every parked wait, least recently reconciled first.
        if full {
            let mut parked: Vec<(Option<DateTime<Utc>>, Key)> = store
                .instance_waits
                .iter()
                .filter(|((waiter, wait_id), wait)| {
                    wait.record.state == WaitState::Pending && parked_on(&store, waiter, wait_id)
                })
                .map(|(key, wait)| (wait.last_reconciled_at, key.clone()))
                .collect();
            parked.sort();
            parked.truncate(limit as usize);
            for (_, key) in parked {
                settle(&mut store, &key, now);
                store
                    .instance_waits
                    .get_mut(&key)
                    .unwrap()
                    .last_reconciled_at = Some(now);
                if store.instance_waits[&key].record.state != WaitState::Pending
                    && stamp(&mut store, &key.0, &key.1, now)
                {
                    woken.insert(key.0.clone());
                }
            }
        }
        Ok(woken.len() as u64)
    }
}
