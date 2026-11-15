//! Independent in-memory implementation of the root lease contract.
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
pub(super) fn lease(store: &Store, token: &InvocationLease, active: bool) -> FenceResult<()> {
    root(store, &token.tenant_id, &token.instance_id, active)?;
    match store.invocation_leases.get(&token.instance_id) {
        Some((current, live)) if current == token && (!active || *live) => Ok(()),
        _ => Err(denied(FenceRejection::LeaseMismatch)),
    }
}
#[async_trait]
impl InvocationFences for InMemoryPersistence {
    async fn get_invocation_lease(
        &self,
        tenant: &str,
        instance: &str,
    ) -> FenceResult<Option<InvocationLeaseState>> {
        let store = self.store.lock().unwrap();
        root(&store, tenant, instance, false)?;
        Ok(store
            .invocation_leases
            .get(instance)
            .map(|(lease, active)| InvocationLeaseState {
                lease: lease.clone(),
                active: *active,
            }))
    }

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
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn lease_ownership() {
        crate::persistence::conformance::invocations::lease_ownership(
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
    async fn concurrent_claims() {
        crate::persistence::conformance::invocations::concurrent_claims(
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
