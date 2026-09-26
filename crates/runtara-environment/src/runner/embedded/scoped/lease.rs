//! One physical runner's bounded invocation ownership. Never steals a live lease.
use super::*;
use runtara_core::persistence::invocations::{
    FenceRejection, InvocationFenceError, InvocationLease,
};

pub(super) struct RootLease {
    persistence: Arc<dyn Persistence>,
    pub token: InvocationLease,
    pub timeout: Duration,
}

impl RootLease {
    pub async fn claim(
        persistence: Arc<dyn Persistence>,
        tenant: &str,
        instance: &str,
        owner: &str,
        timeout: Duration,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(!timeout.is_zero(), "root lease budget exhausted");
        let fences = persistence
            .invocation_fences()
            .ok_or_else(|| anyhow::anyhow!("scoped execution requires invocation fencing"))?;
        let previous =
            tokio::time::timeout(timeout, fences.get_invocation_lease(tenant, instance)).await??;
        anyhow::ensure!(
            previous.as_ref().is_none_or(|lease| !lease.active),
            "another execution owns the root invocation lease"
        );
        let expected = previous.map(|lease| lease.lease.epoch);
        let epoch = expected
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("root invocation epoch exhausted"))?;
        // Retain the exact proposed token before polling the write. If the reply
        // is lost after commit, cleanup can still revoke only this incarnation.
        let owned = Self {
            persistence: persistence.clone(),
            token: InvocationLease {
                tenant_id: tenant.into(),
                instance_id: instance.into(),
                owner: owner.into(),
                epoch,
            },
            timeout,
        };
        match tokio::time::timeout(
            timeout,
            fences.claim_invocation_lease(tenant, instance, owner, expected),
        )
        .await
        {
            Ok(Ok(token)) if token == owned.token => Ok(owned),
            result => {
                owned.release().await?;
                anyhow::bail!("root invocation lease claim failed: {result:?}")
            }
        }
    }

    pub async fn release(&self) -> anyhow::Result<()> {
        match tokio::time::timeout(
            self.timeout,
            self.persistence
                .invocation_fences()
                .unwrap()
                .revoke_invocation_lease(&self.token),
        )
        .await?
        {
            Ok(()) => Ok(()),
            // Suspension/recovery can revoke and replace this owner before its
            // final cleanup reply. Exact-token rejection proves it cannot revoke
            // that replacement. Deletion also removes all execution authority.
            Err(InvocationFenceError::Rejected(
                FenceRejection::LeaseMismatch | FenceRejection::UnknownRoot,
            )) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtara_core::{domain::InstanceStatus, persistence::memory::InMemoryPersistence};

    #[tokio::test]
    async fn physical_ownership_requires_revocation_and_stale_cleanup_preserves_replacement() {
        let p = Arc::new(InMemoryPersistence::new());
        p.register_instance("root", "tenant").await.unwrap();
        let timeout = Duration::from_secs(1);
        assert!(
            RootLease::claim(p.clone(), "tenant", "root", "pending", timeout)
                .await
                .is_err()
        );
        p.update_instance_status("root", InstanceStatus::Running, None)
            .await
            .unwrap();
        let first = RootLease::claim(p.clone(), "tenant", "root", "physical-one", timeout)
            .await
            .unwrap();
        assert!(
            RootLease::claim(p.clone(), "tenant", "root", "competitor", timeout)
                .await
                .is_err()
        );
        let fences = p.invocation_fences().unwrap();
        assert!(
            fences
                .get_invocation_lease("tenant", "root")
                .await
                .unwrap()
                .unwrap()
                .active
        );
        p.update_instance_status("root", InstanceStatus::Suspended, None)
            .await
            .unwrap();
        assert!(
            !fences
                .get_invocation_lease("tenant", "root")
                .await
                .unwrap()
                .unwrap()
                .active
        );
        p.update_instance_status("root", InstanceStatus::Running, None)
            .await
            .unwrap();
        let second = RootLease::claim(p.clone(), "tenant", "root", "physical-two", timeout)
            .await
            .unwrap();
        assert!(second.token.epoch > first.token.epoch);
        first.release().await.unwrap();
        let current = fences
            .get_invocation_lease("tenant", "root")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(current.lease, second.token);
        assert!(current.active);
        second.release().await.unwrap();
        second.release().await.unwrap();
        assert!(
            !fences
                .get_invocation_lease("tenant", "root")
                .await
                .unwrap()
                .unwrap()
                .active
        );
    }
}
