//! Independent in-memory implementation of the atomic invocation contract.
use super::*;
use crate::persistence::invocations::*;

fn denied(reason: FenceRejection) -> InvocationFenceError {
    InvocationFenceError::Rejected(reason)
}
fn root(store: &Store, tenant: &str, instance: &str, running: bool) -> FenceResult<()> {
    validate_identity(tenant)?;
    validate_identity(instance)?;
    let record = store
        .instances
        .get(instance)
        .filter(|r| r.tenant_id == tenant)
        .ok_or_else(|| denied(FenceRejection::UnknownRoot))?;
    if running && record.status != CoreInstanceStatus::Running {
        return Err(denied(FenceRejection::InactiveRoot));
    }
    Ok(())
}
fn lease(store: &Store, token: &InvocationLease, active: bool) -> FenceResult<()> {
    root(store, &token.tenant_id, &token.instance_id, active)?;
    match store.invocation_leases.get(&token.instance_id) {
        Some((current, live)) if current == token && (!active || *live) => Ok(()),
        _ => Err(denied(FenceRejection::LeaseMismatch)),
    }
}
fn attempt(store: &Store, token: &AttemptFence) -> FenceResult<usize> {
    let found = store
        .invocation_attempts
        .iter()
        .rposition(|a| {
            a.fence.lease.instance_id == token.lease.instance_id && a.fence.path == token.path
        })
        .ok_or_else(|| denied(FenceRejection::AttemptMismatch))?;
    if store.invocation_attempts[found].fence != *token {
        return Err(denied(FenceRejection::AttemptMismatch));
    }
    Ok(found)
}
fn checkpoint(
    store: &mut Store,
    token: &AttemptFence,
    checkpoint: &InvocationCheckpoint,
) -> InvocationCheckpointResult {
    if let Some(existing) = store.checkpoints.iter().find(|c| {
        c.instance_id == token.lease.instance_id && c.checkpoint_id == checkpoint.checkpoint_id
    }) {
        return InvocationCheckpointResult {
            found: true,
            state: existing.state.clone(),
        };
    }
    if !checkpoint.state.is_empty() {
        store.checkpoints.push(CheckpointRecord {
            instance_id: token.lease.instance_id.clone(),
            checkpoint_id: checkpoint.checkpoint_id.clone(),
            state: checkpoint.state.clone(),
            created_at: Utc::now(),
        });
        store
            .instances
            .get_mut(&token.lease.instance_id)
            .unwrap()
            .checkpoint_id = Some(checkpoint.checkpoint_id.clone());
    }
    InvocationCheckpointResult {
        found: false,
        state: vec![],
    }
}

#[async_trait]
impl InvocationFences for InMemoryPersistence {
    async fn claim_invocation_lease(
        &self,
        tenant: &str,
        instance: &str,
        owner: &str,
        expected_epoch: Option<i64>,
    ) -> FenceResult<InvocationLease> {
        validate_identity(owner)?;
        if expected_epoch.is_some_and(|epoch| epoch <= 0 || epoch == i64::MAX) {
            return Err(denied(FenceRejection::InvalidIdentity));
        }
        let mut store = self.store.lock().unwrap();
        root(&store, tenant, instance, true)?;
        let next = expected_epoch.unwrap_or(0) + 1;
        if let Some((current, active)) = store.invocation_leases.get(instance) {
            if current.owner == owner && current.epoch == next && *active {
                return Ok(current.clone());
            }
            if *active || Some(current.epoch) != expected_epoch {
                return Err(denied(FenceRejection::LeaseMismatch));
            }
        } else if expected_epoch.is_some() {
            return Err(denied(FenceRejection::LeaseMismatch));
        }
        let token = InvocationLease {
            tenant_id: tenant.into(),
            instance_id: instance.into(),
            owner: owner.into(),
            epoch: next,
        };
        store
            .invocation_leases
            .insert(instance.into(), (token.clone(), true));
        Ok(token)
    }
    async fn revoke_invocation_lease(&self, token: &InvocationLease) -> FenceResult<()> {
        let mut store = self.store.lock().unwrap();
        lease(&store, token, false)?;
        store
            .invocation_leases
            .get_mut(&token.instance_id)
            .unwrap()
            .1 = false;
        Ok(())
    }
    async fn begin_invocation_attempt(
        &self,
        token: &InvocationLease,
        path: &str,
        start_id: &str,
    ) -> FenceResult<InvocationAttempt> {
        validate_identity(path)?;
        validate_identity(start_id)?;
        let mut store = self.store.lock().unwrap();
        lease(&store, token, true)?;
        if let Some(existing) = store
            .invocation_attempts
            .iter()
            .find(|a| a.fence.lease == *token && a.fence.start_id == start_id)
        {
            if existing.fence.path != path {
                return Err(denied(FenceRejection::AttemptMismatch));
            }
            return Ok(existing.clone());
        }
        if let Some(previous) = store
            .invocation_attempts
            .iter()
            .rfind(|a| a.fence.lease.instance_id == token.instance_id && a.fence.path == path)
        {
            if previous.state == AttemptState::Cancelled {
                return Ok(previous.clone());
            }
            if previous.state == AttemptState::Active && previous.fence.lease == *token {
                return Err(denied(FenceRejection::Busy));
            }
        }
        let generation = store
            .invocation_attempts
            .iter()
            .filter(|a| a.fence.lease.instance_id == token.instance_id)
            .map(|a| a.fence.generation)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| denied(FenceRejection::InvalidIdentity))?;
        let result = InvocationAttempt {
            fence: AttemptFence {
                lease: token.clone(),
                path: path.into(),
                start_id: start_id.into(),
                generation,
            },
            state: AttemptState::Active,
        };
        store.invocation_attempts.push(result.clone());
        Ok(result)
    }
    async fn cancel_invocation_attempt(&self, token: &AttemptFence) -> FenceResult<AttemptState> {
        let mut store = self.store.lock().unwrap();
        root(
            &store,
            &token.lease.tenant_id,
            &token.lease.instance_id,
            false,
        )?;
        let index = attempt(&store, token)?;
        let state = &mut store.invocation_attempts[index].state;
        if *state == AttemptState::Active {
            *state = AttemptState::Cancelled;
        }
        Ok(*state)
    }
    async fn settle_invocation_attempt(
        &self,
        token: &AttemptFence,
        write: Option<&InvocationCheckpoint>,
    ) -> FenceResult<InvocationSettlement> {
        if let Some(write) = write {
            validate_identity(&write.checkpoint_id)?;
        }
        let mut store = self.store.lock().unwrap();
        lease(&store, &token.lease, true)?;
        let index = attempt(&store, token)?;
        let mut committed = None;
        if store.invocation_attempts[index].state == AttemptState::Active {
            if let Some(write) = write {
                committed = Some(checkpoint(&mut store, token, write));
            }
            store.invocation_attempts[index].state = AttemptState::Settled;
        }
        Ok(InvocationSettlement {
            state: store.invocation_attempts[index].state,
            checkpoint: committed,
        })
    }
    async fn invocation_checkpoint(
        &self,
        token: &AttemptFence,
        write: &InvocationCheckpoint,
    ) -> FenceResult<InvocationCheckpointResult> {
        validate_identity(&write.checkpoint_id)?;
        let mut store = self.store.lock().unwrap();
        lease(&store, &token.lease, true)?;
        let index = attempt(&store, token)?;
        match store.invocation_attempts[index].state {
            AttemptState::Cancelled => Err(denied(FenceRejection::Cancelled)),
            AttemptState::Settled => Err(denied(FenceRejection::Settled)),
            AttemptState::Active => Ok(checkpoint(&mut store, token, write)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn concurrent_admission() {
        crate::persistence::conformance::invocations::concurrent_admission(
            &InMemoryPersistence::default(),
        )
        .await;
    }
    #[tokio::test]
    async fn lease_ownership() {
        crate::persistence::conformance::invocations::lease_ownership(
            &InMemoryPersistence::default(),
        )
        .await;
    }
    #[tokio::test]
    async fn attempt_admission() {
        crate::persistence::conformance::invocations::attempt_admission(
            &InMemoryPersistence::default(),
        )
        .await;
    }
    #[tokio::test]
    async fn cancellation_replay() {
        crate::persistence::conformance::invocations::cancellation_replay(
            &InMemoryPersistence::default(),
        )
        .await;
    }
    #[tokio::test]
    async fn checkpoint_settlement() {
        crate::persistence::conformance::invocations::checkpoint_settlement(
            &InMemoryPersistence::default(),
        )
        .await;
    }
    #[tokio::test]
    async fn lease_takeover() {
        crate::persistence::conformance::invocations::lease_takeover(
            &InMemoryPersistence::default(),
        )
        .await;
    }
    #[tokio::test]
    async fn cancellation_races() {
        crate::persistence::conformance::invocations::cancellation_races(
            &InMemoryPersistence::default(),
        )
        .await;
    }
    #[tokio::test]
    async fn retention() {
        crate::persistence::conformance::invocations::retention(&InMemoryPersistence::default())
            .await;
    }
}
