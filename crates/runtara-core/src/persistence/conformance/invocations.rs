//! The same behavioral cases run against memory and PostgreSQL.
use crate::domain::InstanceStatus;
use crate::persistence::{Persistence, invocations::*};

async fn root(p: &dyn Persistence) -> (String, InvocationLease) {
    let id = uuid::Uuid::new_v4().to_string();
    p.register_instance(&id, "fence-tenant").await.unwrap();
    p.update_instance_status(&id, InstanceStatus::Running, None)
        .await
        .unwrap();
    assert!(
        p.invocation_fences()
            .unwrap()
            .get_invocation_lease("fence-tenant", &id)
            .await
            .unwrap()
            .is_none()
    );
    let lease = p
        .invocation_fences()
        .unwrap()
        .claim_invocation_lease("fence-tenant", &id, "launch-one", None)
        .await
        .unwrap();
    (id, lease)
}
fn rejected<T: std::fmt::Debug>(result: FenceResult<T>, reason: FenceRejection) {
    assert!(
        matches!(result,Err(InvocationFenceError::Rejected(actual)) if actual == reason),
        "expected {reason:?}, got {result:?}"
    );
}
/// Leases require tenant ownership, running roots and exact revoked epochs.
pub async fn lease_ownership(p: &dyn Persistence) {
    let (id, lease) = root(p).await;
    let f = p.invocation_fences().unwrap();
    assert_eq!(
        f.claim_invocation_lease("fence-tenant", &id, "launch-one", None)
            .await
            .unwrap(),
        lease
    );
    rejected(
        f.claim_invocation_lease("other-tenant", &id, "attack", None)
            .await,
        FenceRejection::UnknownRoot,
    );
    rejected(
        f.claim_invocation_lease("fence-tenant", &id, "launch-two", Some(1))
            .await,
        FenceRejection::LeaseMismatch,
    );
    for epoch in [0, -1, i64::MAX] {
        rejected(
            f.claim_invocation_lease("fence-tenant", &id, "launch-two", Some(epoch))
                .await,
            FenceRejection::InvalidIdentity,
        );
    }
    f.revoke_invocation_lease(&lease).await.unwrap();
    f.revoke_invocation_lease(&lease).await.unwrap();
    rejected(
        f.claim_invocation_lease("fence-tenant", &id, "launch-two", None)
            .await,
        FenceRejection::LeaseMismatch,
    );
    assert_eq!(
        f.get_invocation_lease("fence-tenant", &id).await.unwrap(),
        Some(InvocationLeaseState {
            lease: lease.clone(),
            active: false
        })
    );
    rejected(
        f.get_invocation_lease("other-tenant", &id).await,
        FenceRejection::UnknownRoot,
    );
    let next = f
        .claim_invocation_lease("fence-tenant", &id, "launch-two", Some(1))
        .await
        .unwrap();
    assert_eq!(next.epoch, 2);
    rejected(
        f.revoke_invocation_lease(&lease).await,
        FenceRejection::LeaseMismatch,
    );
    assert_eq!(
        f.claim_invocation_lease("fence-tenant", &id, "launch-two", Some(1))
            .await
            .unwrap(),
        next
    );
    assert_eq!(
        f.get_invocation_lease("fence-tenant", &id).await.unwrap(),
        Some(InvocationLeaseState {
            lease: next.clone(),
            active: true
        })
    );
    p.update_instance_status(&id, InstanceStatus::Completed, None)
        .await
        .unwrap();
    // A root that left Running no longer has an active owner, and a revoke
    // of its last one still succeeds.
    assert!(
        !f.get_invocation_lease("fence-tenant", &id)
            .await
            .unwrap()
            .unwrap()
            .active
    );
    f.revoke_invocation_lease(&next).await.unwrap();
}

/// A revoked lease is replaced only at its exact epoch, leaving Running
/// revokes the owner, and a resumed root is claimed at a new epoch.
pub async fn lease_takeover(p: &dyn Persistence) {
    let (id, lease) = root(p).await;
    let f = p.invocation_fences().unwrap();
    f.revoke_invocation_lease(&lease).await.unwrap();
    let next = f
        .claim_invocation_lease("fence-tenant", &id, "new", Some(lease.epoch))
        .await
        .unwrap();
    assert!(next.epoch > lease.epoch);
    rejected(
        f.revoke_invocation_lease(&lease).await,
        FenceRejection::LeaseMismatch,
    );
    p.update_instance_status(&id, InstanceStatus::Suspended, None)
        .await
        .unwrap();
    assert!(
        !f.get_invocation_lease("fence-tenant", &id)
            .await
            .unwrap()
            .unwrap()
            .active
    );
    rejected(
        f.claim_invocation_lease("fence-tenant", &id, "parked", Some(next.epoch))
            .await,
        FenceRejection::InactiveRoot,
    );
    p.update_instance_status(&id, InstanceStatus::Running, None)
        .await
        .unwrap();
    let resumed = f
        .claim_invocation_lease("fence-tenant", &id, "resumed", Some(next.epoch))
        .await
        .unwrap();
    assert!(resumed.epoch > next.epoch);
}

/// A deleted root's lease goes with it: an instance registered again under
/// the same id starts from no lease.
pub async fn retention(p: &dyn Persistence) {
    let (id, lease) = root(p).await;
    let f = p.invocation_fences().unwrap();
    assert_eq!(
        p.delete_instances_batch(std::slice::from_ref(&id))
            .await
            .unwrap(),
        1
    );
    p.register_instance(&id, "replacement-tenant")
        .await
        .unwrap();
    p.update_instance_status(&id, InstanceStatus::Running, None)
        .await
        .unwrap();
    assert!(
        f.get_invocation_lease("replacement-tenant", &id)
            .await
            .unwrap()
            .is_none()
    );
    let next = f
        .claim_invocation_lease("replacement-tenant", &id, "new", None)
        .await
        .unwrap();
    assert_eq!(next.epoch, 1);
    rejected(
        f.revoke_invocation_lease(&lease).await,
        FenceRejection::UnknownRoot,
    );
}

/// Conflicting claims of a revoked lease have exactly one winner.
pub async fn concurrent_claims(p: &dyn Persistence) {
    let (id, lease) = root(p).await;
    let f = p.invocation_fences().unwrap();
    f.revoke_invocation_lease(&lease).await.unwrap();
    let (a, b) = tokio::join!(
        f.claim_invocation_lease("fence-tenant", &id, "owner-a", Some(lease.epoch)),
        f.claim_invocation_lease("fence-tenant", &id, "owner-b", Some(lease.epoch))
    );
    match (a, b) {
        (Ok(_), Err(InvocationFenceError::Rejected(FenceRejection::LeaseMismatch)))
        | (Err(InvocationFenceError::Rejected(FenceRejection::LeaseMismatch)), Ok(_)) => {}
        pair => panic!("lease claims must have exactly one winner: {pair:?}"),
    }
}
