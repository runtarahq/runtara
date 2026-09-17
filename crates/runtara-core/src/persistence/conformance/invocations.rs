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
fn write(key: &str, bytes: &[u8]) -> InvocationCheckpoint {
    InvocationCheckpoint {
        checkpoint_id: key.into(),
        state: bytes.into(),
    }
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
    rejected(
        f.begin_invocation_attempt(&next, "path", "start").await,
        FenceRejection::InactiveRoot,
    );
    f.revoke_invocation_lease(&next).await.unwrap();
}

/// Admission is idempotent, enforces one live owner, and never memoizes success.
pub async fn attempt_admission(p: &dyn Persistence) {
    let (_, lease) = root(p).await;
    let f = p.invocation_fences().unwrap();
    let first = f
        .begin_invocation_attempt(&lease, "root/loop/7/call", "one")
        .await
        .unwrap();
    assert_eq!(
        f.begin_invocation_attempt(&lease, &first.fence.path, "one")
            .await
            .unwrap(),
        first
    );
    rejected(
        f.begin_invocation_attempt(&lease, &first.fence.path, "two")
            .await,
        FenceRejection::Busy,
    );
    rejected(
        f.begin_invocation_attempt(&lease, "sibling", "one").await,
        FenceRejection::AttemptMismatch,
    );
    let sibling = f
        .begin_invocation_attempt(&lease, "root/loop/8/call", "sibling")
        .await
        .unwrap();
    assert_ne!(first.fence, sibling.fence);
    assert_eq!(
        f.settle_invocation_attempt(&first.fence, None)
            .await
            .unwrap()
            .state,
        AttemptState::Settled
    );
    let second = f
        .begin_invocation_attempt(&lease, &first.fence.path, "two")
        .await
        .unwrap();
    assert!(second.fence.generation > first.fence.generation);
    assert_eq!(
        f.begin_invocation_attempt(&lease, &first.fence.path, "one")
            .await
            .unwrap()
            .state,
        AttemptState::Settled
    );
    rejected(
        f.cancel_invocation_attempt(&first.fence).await,
        FenceRejection::AttemptMismatch,
    );
    assert_eq!(
        f.settle_invocation_attempt(&second.fence, None)
            .await
            .unwrap()
            .state,
        AttemptState::Settled
    );
    // Full path equality survives values larger than a PostgreSQL btree key.
    let long = "深".repeat(1300);
    assert_eq!(
        f.begin_invocation_attempt(&lease, &long, "long")
            .await
            .unwrap()
            .fence
            .path,
        long
    );
    for path in ["".into(), "bad\0path".into(), "x".repeat(4097)] {
        rejected(
            f.begin_invocation_attempt(&lease, &path, "invalid").await,
            FenceRejection::InvalidIdentity,
        );
    }
}

/// A cancellation winner fences writes and survives replay without affecting siblings.
pub async fn cancellation_replay(p: &dyn Persistence) {
    let (id, lease) = root(p).await;
    let f = p.invocation_fences().unwrap();
    let target = f
        .begin_invocation_attempt(&lease, "parent/call", "one")
        .await
        .unwrap();
    let sibling = f
        .begin_invocation_attempt(&lease, "parent/call-other", "sibling")
        .await
        .unwrap();
    let mut forged = target.fence.clone();
    forged.lease.tenant_id = "other-tenant".into();
    rejected(
        f.cancel_invocation_attempt(&forged).await,
        FenceRejection::UnknownRoot,
    );
    forged = target.fence.clone();
    forged.start_id = "forged".into();
    rejected(
        f.invocation_checkpoint(&forged, &write("checkpoint", b"bad"))
            .await,
        FenceRejection::AttemptMismatch,
    );
    assert_eq!(
        f.cancel_invocation_attempt(&target.fence).await.unwrap(),
        AttemptState::Cancelled
    );
    assert_eq!(
        f.cancel_invocation_attempt(&target.fence).await.unwrap(),
        AttemptState::Cancelled
    );
    rejected(
        f.invocation_checkpoint(&target.fence, &write("checkpoint", b"late"))
            .await,
        FenceRejection::Cancelled,
    );
    assert_eq!(
        f.settle_invocation_attempt(&target.fence, Some(&write("checkpoint", b"late")))
            .await
            .unwrap()
            .state,
        AttemptState::Cancelled
    );
    assert!(
        p.load_checkpoint(&id, "checkpoint")
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        f.settle_invocation_attempt(&sibling.fence, None)
            .await
            .unwrap()
            .state,
        AttemptState::Settled
    );
    f.revoke_invocation_lease(&lease).await.unwrap();
    let next = f
        .claim_invocation_lease("fence-tenant", &id, "replay", Some(lease.epoch))
        .await
        .unwrap();
    assert_eq!(
        f.begin_invocation_attempt(&next, &target.fence.path, "replay-start")
            .await
            .unwrap(),
        InvocationAttempt {
            state: AttemptState::Cancelled,
            ..target
        }
    );
}

/// Settlement and logical checkpoint commit together, preserving first-write replay bytes.
pub async fn checkpoint_settlement(p: &dyn Persistence) {
    let (id, lease) = root(p).await;
    let f = p.invocation_fences().unwrap();
    let target = f
        .begin_invocation_attempt(&lease, "child", "one")
        .await
        .unwrap();
    let fence = &target.fence;
    assert!(
        !f.invocation_checkpoint(fence, &write("child::step", b""))
            .await
            .unwrap()
            .found
    );
    assert!(
        p.load_checkpoint(&id, "child::step")
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        p.get_instance(&id).await.unwrap().unwrap().checkpoint_id,
        None
    );
    assert!(
        !f.invocation_checkpoint(fence, &write("child::step", b"first"))
            .await
            .unwrap()
            .found
    );
    assert_eq!(
        f.invocation_checkpoint(fence, &write("child::step", b"second"))
            .await
            .unwrap(),
        InvocationCheckpointResult {
            found: true,
            state: b"first".to_vec()
        }
    );
    assert_eq!(
        p.get_instance(&id)
            .await
            .unwrap()
            .unwrap()
            .checkpoint_id
            .as_deref(),
        Some("child::step")
    );
    assert_eq!(
        f.settle_invocation_attempt(fence, Some(&write("child::finish", b"result")))
            .await
            .unwrap()
            .state,
        AttemptState::Settled
    );
    assert_eq!(
        f.cancel_invocation_attempt(fence).await.unwrap(),
        AttemptState::Settled
    );
    assert_eq!(
        f.settle_invocation_attempt(fence, Some(&write("late", b"bad")))
            .await
            .unwrap()
            .state,
        AttemptState::Settled
    );
    assert!(p.load_checkpoint(&id, "late").await.unwrap().is_none());
    rejected(
        f.invocation_checkpoint(fence, &write("child::finish", b"late"))
            .await,
        FenceRejection::Settled,
    );
    assert_eq!(
        p.load_checkpoint(&id, "child::finish")
            .await
            .unwrap()
            .unwrap()
            .state,
        b"result"
    );
    let replay = f
        .begin_invocation_attempt(&lease, "another-call", "replay")
        .await
        .unwrap();
    let settled = f
        .settle_invocation_attempt(&replay.fence, Some(&write("child::finish", b"different")))
        .await
        .unwrap();
    assert_eq!(
        settled.checkpoint,
        Some(InvocationCheckpointResult {
            found: true,
            state: b"result".to_vec()
        })
    );
}

/// Revocation prevents old owners from publishing while a new lease reclaims work.
pub async fn lease_takeover(p: &dyn Persistence) {
    let (id, lease) = root(p).await;
    let f = p.invocation_fences().unwrap();
    let old = f
        .begin_invocation_attempt(&lease, "child", "one")
        .await
        .unwrap();
    f.revoke_invocation_lease(&lease).await.unwrap();
    rejected(
        f.invocation_checkpoint(&old.fence, &write("old", b"bad"))
            .await,
        FenceRejection::LeaseMismatch,
    );
    let next = f
        .claim_invocation_lease("fence-tenant", &id, "new", Some(lease.epoch))
        .await
        .unwrap();
    let current = f
        .begin_invocation_attempt(&next, "child", "new-start")
        .await
        .unwrap();
    assert!(current.fence.generation > old.fence.generation);
    rejected(
        f.settle_invocation_attempt(&old.fence, Some(&write("old", b"bad")))
            .await,
        FenceRejection::LeaseMismatch,
    );
    rejected(
        f.cancel_invocation_attempt(&old.fence).await,
        FenceRejection::AttemptMismatch,
    );
    p.update_instance_status(&id, InstanceStatus::Suspended, None)
        .await
        .unwrap();
    assert_eq!(
        f.cancel_invocation_attempt(&current.fence).await.unwrap(),
        AttemptState::Cancelled
    );
    rejected(
        f.invocation_checkpoint(&current.fence, &write("old", b"bad"))
            .await,
        FenceRejection::InactiveRoot,
    );
    assert!(p.load_checkpoint(&id, "old").await.unwrap().is_none());
}

/// Races have one durable winner, including the completion checkpoint bytes.
pub async fn cancellation_races(p: &dyn Persistence) {
    let (id, lease) = root(p).await;
    let f = p.invocation_fences().unwrap();
    for i in 0..24 {
        let key = format!("child-{i}");
        let attempt = f
            .begin_invocation_attempt(&lease, &key, &key)
            .await
            .unwrap();
        let value = write(&key, b"completed");
        let (cancel, settle) = if i % 2 == 0 {
            tokio::join!(
                f.cancel_invocation_attempt(&attempt.fence),
                f.settle_invocation_attempt(&attempt.fence, Some(&value))
            )
        } else {
            let (settle, cancel) = tokio::join!(
                f.settle_invocation_attempt(&attempt.fence, Some(&value)),
                f.cancel_invocation_attempt(&attempt.fence)
            );
            (cancel, settle)
        };
        let cancel = cancel.unwrap();
        let settle = settle.unwrap().state;
        assert_eq!(cancel, settle);
        let checkpoint = p.load_checkpoint(&id, &key).await.unwrap();
        match cancel {
            AttemptState::Cancelled => assert!(checkpoint.is_none()),
            AttemptState::Settled => assert_eq!(checkpoint.unwrap().state, b"completed"),
            AttemptState::Active => panic!("race did not settle"),
        }
    }
}

/// Retention deletes leases and tombstones with their root, including tenant reuse.
pub async fn retention(p: &dyn Persistence) {
    let (id, lease) = root(p).await;
    let f = p.invocation_fences().unwrap();
    let attempt = f
        .begin_invocation_attempt(&lease, "child", "one")
        .await
        .unwrap();
    f.cancel_invocation_attempt(&attempt.fence).await.unwrap();
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
    let next = f
        .claim_invocation_lease("replacement-tenant", &id, "new", None)
        .await
        .unwrap();
    assert_eq!(
        f.begin_invocation_attempt(&next, "child", "one")
            .await
            .unwrap()
            .state,
        AttemptState::Active
    );
    rejected(
        f.cancel_invocation_attempt(&attempt.fence).await,
        FenceRejection::UnknownRoot,
    );
}

/// Concurrent admission allocates once and conflicting lease claims have one winner.
pub async fn concurrent_admission(p: &dyn Persistence) {
    let (id, lease) = root(p).await;
    let f = p.invocation_fences().unwrap();
    let (a, b) = tokio::join!(
        f.begin_invocation_attempt(&lease, "path", "same-start"),
        f.begin_invocation_attempt(&lease, "path", "same-start")
    );
    assert_eq!(a.unwrap(), b.unwrap());
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

#[path = "invocation_writes.rs"]
mod writes;
pub use writes::{child_write_boundaries, child_write_rejections, child_write_semantics};
