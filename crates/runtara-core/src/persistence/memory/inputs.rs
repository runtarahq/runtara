//! Input transitions share the store lock with every lifecycle and raw write.
use super::*;
use crate::persistence::inputs::*;
use crate::persistence::invocations::{AttemptFence, AttemptState};

pub(super) struct InputPark {
    pub signals: Vec<String>,
    /// Also set when a timer claims this park. Further responses must not
    /// shorten its launch lease or failed-launch retry deadline.
    pub wake_scheduled: bool,
}

fn wake_candidates(store: &Store, instance: &str, all_accepted: bool) -> Vec<(String, String)> {
    let Some(root) = store.instances.get(instance) else {
        return vec![];
    };
    if root.status != CoreInstanceStatus::Suspended
        || root.termination_reason.as_deref() != Some("waiting_signal")
    {
        return vec![];
    }
    let Some(park) = store.input_parks.get(instance) else {
        return vec![];
    };
    store
        .input_requests
        .iter()
        .filter(|(_, r)| {
            r.instance_id == instance
                && (all_accepted || r.wake_pending)
                && matches!(r.state, InputState::Accepted { .. })
                && park.signals.contains(&r.spec.signal_id)
                && owner_live(store, r)
        })
        .map(|(key, _)| key.clone())
        .collect()
}

/// Caller holds the root/store lock. A fresh park examines retained responses
/// even if an earlier park already consumed their wake intent.
pub(super) fn schedule_accepted(
    store: &mut Store,
    instance: &str,
    now: DateTime<Utc>,
    all_accepted: bool,
) -> bool {
    let candidates = wake_candidates(store, instance, all_accepted);
    if candidates.is_empty() {
        return false;
    }
    let park = store.input_parks.get_mut(instance).unwrap();
    if !park.wake_scheduled {
        let root = store.instances.get_mut(instance).unwrap();
        root.sleep_until = Some(root.sleep_until.map_or(now, |deadline| deadline.min(now)));
        root.wake_reason = Some(crate::domain::WakeReason::CustomSignal);
        park.wake_scheduled = true;
    }
    for key in candidates {
        store.input_requests.get_mut(&key).unwrap().wake_pending = false;
    }
    true
}

fn root<'a>(store: &'a Store, tenant: &str, instance: &str) -> InputResult<&'a InstanceRecord> {
    store
        .instances
        .get(instance)
        .filter(|r| r.tenant_id == tenant)
        .ok_or(InputError::NotFound)
}

fn authority(store: &Store, owner: &InputAuthority) -> InputResult<()> {
    let instance = root(store, owner.tenant_id(), owner.instance_id())?;
    if instance.status != CoreInstanceStatus::Running {
        return Err(InputError::Inactive);
    }
    match owner {
        InputAuthority::Root { instance_id, .. } => {
            if store.invocation_leases.contains_key(instance_id) {
                return Err(InputError::FenceRejected);
            }
            Ok(())
        }
        InputAuthority::LeasedRoot(lease) => {
            super::invocations::lease(store, lease, true).map_err(|_| InputError::FenceRejected)
        }
        InputAuthority::Invocation(fence) => {
            super::invocations::active_attempt(store, fence).map_err(|_| InputError::FenceRejected)
        }
    }
}

fn owner_live(store: &Store, request: &InputRequest) -> bool {
    request.invocation_path.is_empty()
        || store
            .invocation_attempts
            .iter()
            .rev()
            .find(|a| {
                a.fence.lease.instance_id == request.instance_id
                    && a.fence.path == request.invocation_path
            })
            .is_some_and(|a| a.state == AttemptState::Active)
}

fn actionable(store: &Store, request: &InputRequest, now: DateTime<Utc>) -> bool {
    request.open_at(now)
        && !store.instances[&request.instance_id].status.is_terminal()
        && owner_live(store, request)
}

fn replay(
    store: &Store,
    instance: &str,
    request: &str,
    operation: &str,
    identity: InputReplayIdentity<'_>,
) -> InputResult<Option<InputReceipt>> {
    for r in store
        .input_requests
        .values()
        .filter(|r| r.instance_id == instance)
    {
        if let InputState::Accepted { receipt } = &r.state
            && receipt.operation_id == operation
        {
            if receipt.request_id != request || !identity.matches(receipt) {
                return Err(InputError::OperationConflict);
            }
            return Ok(Some(receipt.clone()));
        }
    }
    Ok(None)
}

pub(super) fn close_root(store: &mut Store, instance: &str, now: DateTime<Utc>) {
    store.input_parks.remove(instance);
    if let Some(root) = store.instances.get_mut(instance) {
        root.sleep_until = None;
        root.wake_reason = None;
    }
    for request in store
        .input_requests
        .values_mut()
        .filter(|r| r.instance_id == instance)
    {
        request.close(InputClosure::InstanceTerminated, now);
        request.wake_pending = false;
    }
}

pub(super) fn close_invocation(store: &mut Store, fence: &AttemptFence, reason: InputClosure) {
    let mut family = std::collections::HashSet::from([fence.path.clone()]);
    loop {
        let descendants: Vec<_> = store
            .invocation_parents
            .iter()
            .filter(|((instance, path), parent)| {
                instance == &fence.lease.instance_id
                    && !family.contains(path)
                    && parent.as_ref().is_some_and(|p| family.contains(p))
            })
            .map(|((_, path), _)| path.clone())
            .collect();
        if descendants.is_empty() {
            break;
        }
        family.extend(descendants);
    }
    // Fencing every descendant also prevents it from registering another wait
    // after its ancestor has closed. A sibling with a similar name is untouched.
    for attempt in &mut store.invocation_attempts {
        if attempt.fence.lease.instance_id == fence.lease.instance_id
            && attempt.fence.path != fence.path
            && family.contains(&attempt.fence.path)
            && attempt.state == AttemptState::Active
        {
            attempt.state = AttemptState::Cancelled;
        }
    }
    for request in store
        .input_requests
        .values_mut()
        .filter(|r| r.instance_id == fence.lease.instance_id && family.contains(&r.invocation_path))
    {
        request.close(reason, Utc::now());
        request.wake_pending = false;
    }
}

#[async_trait]
impl InputRequests for InMemoryPersistence {
    async fn reconcile_input_wakes(&self, limit: u32) -> InputResult<u64> {
        let mut store = self.store.lock().unwrap();
        let mut roots: Vec<_> = store
            .input_parks
            .keys()
            .filter(|id| !wake_candidates(&store, id, false).is_empty())
            .cloned()
            .collect();
        roots.sort();
        let mut count = 0;
        for id in roots.into_iter().take(limit as usize) {
            count += u64::from(schedule_accepted(&mut store, &id, Utc::now(), false));
        }
        Ok(count)
    }

    async fn register_input(
        &self,
        owner: &InputAuthority,
        spec: &InputRequestSpec,
    ) -> InputResult<InputRequest> {
        spec.validate()?;
        let mut store = self.store.lock().unwrap();
        authority(&store, owner)?;
        let key = (owner.instance_id().to_owned(), spec.request_id());
        if let Some(existing) = store.input_requests.get_mut(&key) {
            if existing.spec.signal_id != spec.signal_id
                || existing.invocation_path != owner.invocation_path()
            {
                return Err(InputError::IdentityConflict);
            }
            if let InputAuthority::Invocation(fence) = owner {
                existing.fence = Some(fence.clone());
            }
            return Ok(existing.clone());
        }
        if store
            .custom_signals
            .contains_key(&(owner.instance_id().into(), spec.signal_id.clone()))
        {
            return Err(InputError::RawSignalConflict);
        }
        let mut request = InputRequest {
            tenant_id: owner.tenant_id().into(),
            instance_id: owner.instance_id().into(),
            request_id: key.1.clone(),
            invocation_path: owner.invocation_path().into(),
            fence: match owner {
                InputAuthority::Invocation(fence) => Some(fence.clone()),
                _ => None,
            },
            spec: spec.clone(),
            created_at: Utc::now(),
            state: InputState::Open,
            wake_pending: false,
        };
        if !request.open_at(request.created_at) {
            request.close(InputClosure::Expired, request.created_at);
        }
        store.input_requests.insert(key, request.clone());
        Ok(request)
    }

    async fn get_input(
        &self,
        tenant: &str,
        instance: &str,
        request: &str,
    ) -> InputResult<InputRequest> {
        let store = self.store.lock().unwrap();
        root(&store, tenant, instance)?;
        store
            .input_requests
            .get(&(instance.into(), request.into()))
            .cloned()
            .ok_or(InputError::NotFound)
    }

    async fn replay_input(
        &self,
        tenant: &str,
        instance: &str,
        request: &str,
        operation: &str,
        identity: InputReplayIdentity<'_>,
    ) -> InputResult<Option<InputReceipt>> {
        validate_operation_id(operation)?;
        let store = self.store.lock().unwrap();
        root(&store, tenant, instance)?;
        replay(&store, instance, request, operation, identity)
    }

    async fn poll_input(&self, owner: &InputAuthority, request: &str) -> InputResult<InputRequest> {
        let mut store = self.store.lock().unwrap();
        authority(&store, owner)?;
        let r = store
            .input_requests
            .get_mut(&(owner.instance_id().into(), request.into()))
            .ok_or(InputError::NotFound)?;
        if r.invocation_path != owner.invocation_path() {
            return Err(InputError::FenceRejected);
        }
        let now = Utc::now();
        if r.spec.deadline.is_some_and(|deadline| deadline <= now) {
            r.close(InputClosure::Expired, now);
        }
        Ok(r.clone())
    }

    async fn accept_input(
        &self,
        tenant: &str,
        instance: &str,
        response: &ValidatedInputResponse,
    ) -> InputResult<InputReceipt> {
        let mut store = self.store.lock().unwrap();
        let root = root(&store, tenant, instance)?;
        let id = response.spec().request_id();
        if let Some(receipt) = replay(
            &store,
            instance,
            &id,
            response.operation_id(),
            response.replay_identity(),
        )? {
            return Ok(receipt);
        }
        if root.status.is_terminal() {
            return Err(InputError::Inactive);
        }
        let key = (instance.into(), id.clone());
        let request = store.input_requests.get(&key).ok_or(InputError::NotFound)?;
        if request.spec != *response.spec() {
            return Err(InputError::IdentityConflict);
        }
        if matches!(request.state, InputState::Accepted { .. }) {
            return Err(InputError::AlreadyAnswered);
        }
        let now = Utc::now();
        if !request.open_at(now) || !owner_live(&store, request) {
            return Err(InputError::Inactive);
        }
        let receipt = InputReceipt {
            receipt_id: uuid::Uuid::new_v4().to_string(),
            operation_id: response.operation_id().into(),
            request_id: id,
            accepted_at: now,
            payload: response.payload().to_vec(),
            acceptance_context: response.acceptance_context().map(<[u8]>::to_vec),
        };
        let request = store.input_requests.get_mut(&key).unwrap();
        request.state = InputState::Accepted {
            receipt: receipt.clone(),
        };
        request.wake_pending = true;
        schedule_accepted(&mut store, instance, now, false);
        Ok(receipt)
    }

    async fn close_input(
        &self,
        owner: &InputAuthority,
        request: &str,
        reason: InputClosure,
    ) -> InputResult<InputRequest> {
        let mut store = self.store.lock().unwrap();
        authority(&store, owner)?;
        let r = store
            .input_requests
            .get_mut(&(owner.instance_id().into(), request.into()))
            .ok_or(InputError::NotFound)?;
        if r.invocation_path != owner.invocation_path() {
            return Err(InputError::FenceRejected);
        }
        let now = Utc::now();
        if reason != InputClosure::Expired
            || r.spec.deadline.is_some_and(|deadline| deadline <= now)
        {
            r.close(reason, now);
        }
        Ok(r.clone())
    }

    async fn list_inputs(
        &self,
        tenant: &str,
        instances: &[String],
        offset: u64,
        limit: u32,
    ) -> InputResult<InputRequestPage> {
        let store = self.store.lock().unwrap();
        for instance in instances {
            root(&store, tenant, instance)?;
        }
        let now = Utc::now();
        let mut requests: Vec<_> = store
            .input_requests
            .values()
            .filter(|r| {
                instances.contains(&r.instance_id)
                    && r.tenant_id == tenant
                    && actionable(&store, r, now)
            })
            .cloned()
            .collect();
        requests.sort_by(|a, b| {
            (a.created_at, &a.request_id, &a.instance_id).cmp(&(
                b.created_at,
                &b.request_id,
                &b.instance_id,
            ))
        });
        let total_count = requests.len() as u64;
        let requests = requests
            .into_iter()
            .skip(usize::try_from(offset).unwrap_or(usize::MAX))
            .take(limit as usize)
            .collect();
        Ok(InputRequestPage {
            requests,
            total_count,
        })
    }

    async fn instances_with_open_inputs(
        &self,
        tenant: &str,
        instances: &[String],
    ) -> InputResult<std::collections::BTreeSet<String>> {
        let store = self.store.lock().unwrap();
        let ids: std::collections::BTreeSet<_> = instances.iter().collect();
        for instance in &ids {
            root(&store, tenant, instance)?;
        }
        let now = Utc::now();
        Ok(store
            .input_requests
            .values()
            .filter(|r| {
                ids.contains(&r.instance_id) && r.tenant_id == tenant && actionable(&store, r, now)
            })
            .map(|r| r.instance_id.clone())
            .collect())
    }
}
