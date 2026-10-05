//! Durability lifecycle cases shared by every durable backend: first-wins
//! result checkpoints, root execution ownership, and owned lifecycle
//! transitions (see `docs/durability-changes.md`).
use crate::domain::{InstanceStatus, SignalType};
use crate::lifecycle::{ParkOutcome, ParkReason, ParkRequest};
use crate::persistence::invocations::{InvocationFences, InvocationLease};
use crate::persistence::{CheckpointWrite, CompleteInstanceParams, ParkTargets, Persistence};

async fn running(p: &dyn Persistence, name: &str) -> String {
    let id = format!("durability-{name}-{}", uuid::Uuid::new_v4());
    p.register_instance(&id, "durability").await.unwrap();
    p.update_instance_status(&id, InstanceStatus::Running, None)
        .await
        .unwrap();
    id
}

/// The first recorded bytes win; a later write gets them back, writes
/// nothing, and leaves the instance's checkpoint pointer alone.
pub async fn record_checkpoint_first_write_wins(p: &dyn Persistence) {
    let id = running(p, "first-wins").await;
    assert_eq!(
        p.record_checkpoint(&id, "step-a", b"first").await.unwrap(),
        CheckpointWrite::Recorded
    );
    let instance = p.get_instance(&id).await.unwrap().unwrap();
    assert_eq!(instance.checkpoint_id.as_deref(), Some("step-a"));

    p.record_checkpoint(&id, "step-b", b"other").await.unwrap();
    assert_eq!(
        p.record_checkpoint(&id, "step-a", b"second").await.unwrap(),
        CheckpointWrite::Existing(b"first".to_vec()),
        "a second write to a recorded key must adopt the first bytes"
    );
    let stored = p.load_checkpoint(&id, "step-a").await.unwrap().unwrap();
    assert_eq!(stored.state, b"first");
    let instance = p.get_instance(&id).await.unwrap().unwrap();
    assert_eq!(
        instance.checkpoint_id.as_deref(),
        Some("step-b"),
        "a losing write must not move the checkpoint pointer"
    );
    p.delete_instances_batch(&[id]).await.unwrap();
}

/// Concurrent writers to one key: exactly one records, every other gets the
/// winner's bytes, and the stored row is the winner's.
pub async fn record_checkpoint_concurrent_writers_agree<P: Persistence + 'static>(
    p: std::sync::Arc<P>,
) {
    let id = running(p.as_ref(), "concurrent").await;
    let writers: Vec<_> = (0..16u8)
        .map(|n| {
            let p = p.clone();
            let id = id.clone();
            tokio::spawn(async move {
                let bytes = vec![n; 8];
                let write = p.record_checkpoint(&id, "race", &bytes).await.unwrap();
                (bytes, write)
            })
        })
        .collect();
    let mut winners = Vec::new();
    let mut adopted = Vec::new();
    for writer in writers {
        match writer.await.unwrap() {
            (bytes, CheckpointWrite::Recorded) => winners.push(bytes),
            (_, CheckpointWrite::Existing(bytes)) => adopted.push(bytes),
        }
    }
    assert_eq!(winners.len(), 1, "exactly one concurrent writer records");
    let winner = winners.pop().unwrap();
    assert!(adopted.iter().all(|bytes| *bytes == winner));
    let stored = p.load_checkpoint(&id, "race").await.unwrap().unwrap();
    assert_eq!(stored.state, winner);
    p.delete_instances_batch(&[id]).await.unwrap();
}

fn fences(p: &dyn Persistence) -> &dyn InvocationFences {
    p.invocation_fences()
        .expect("a durable backend provides invocation fences")
}

/// Claim the next root lease for a running instance, as a launch does.
async fn claim(p: &dyn Persistence, id: &str, owner: &str) -> InvocationLease {
    let previous = fences(p)
        .get_invocation_lease("durability", id)
        .await
        .unwrap()
        .map(|state| state.lease.epoch);
    fences(p)
        .claim_invocation_lease("durability", id, owner, previous)
        .await
        .unwrap()
}

/// Replace the execution holding `id`: recovery moves it out of `running`
/// (revoking the old lease), and the replacement promotes and claims.
async fn relaunch(p: &dyn Persistence, id: &str, owner: &str) -> InvocationLease {
    p.update_instance_status(id, InstanceStatus::Suspended, None)
        .await
        .unwrap();
    p.update_instance_status(id, InstanceStatus::Running, None)
        .await
        .unwrap();
    claim(p, id, owner).await
}

fn timer_park() -> ParkRequest {
    ParkRequest {
        reason: ParkReason::Timer,
        deadline: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
    }
}

const NO_TARGETS: ParkTargets<'static> = ParkTargets {
    signal_ids: &[],
    wait_ids: &[],
};

/// A park committed by an execution is recognised when that execution
/// retries it after a lost acknowledgement: the retry writes nothing.
pub async fn owned_park_retry_is_already_parked(p: &dyn Persistence) {
    let id = running(p, "park-retry").await;
    let lease = claim(p, &id, "runner-a").await;
    assert_eq!(
        p.park_execution(&lease, timer_park(), NO_TARGETS)
            .await
            .unwrap(),
        ParkOutcome::Parked
    );
    let parked = p.get_instance(&id).await.unwrap().unwrap();
    assert_eq!(parked.status, InstanceStatus::Suspended);
    let events = p.count_events(&id, &Default::default()).await.unwrap();

    let retry = ParkRequest {
        reason: ParkReason::Timer,
        deadline: Some(chrono::Utc::now() + chrono::Duration::hours(5)),
    };
    assert_eq!(
        p.park_execution(&lease, retry, NO_TARGETS).await.unwrap(),
        ParkOutcome::AlreadyParked
    );
    let after = p.get_instance(&id).await.unwrap().unwrap();
    assert_eq!(after.status, InstanceStatus::Suspended);
    assert_eq!(
        after.sleep_until, parked.sleep_until,
        "a retry must not move the wake"
    );
    assert_eq!(
        p.count_events(&id, &Default::default()).await.unwrap(),
        events,
        "a retry must not append events"
    );
    p.delete_instances_batch(&[id]).await.unwrap();
}

/// After a replacement execution owns the root, the superseded execution
/// can neither park nor complete it; the replacement still can.
pub async fn superseded_execution_cannot_park_or_complete(p: &dyn Persistence) {
    let id = running(p, "stale").await;
    let stale = claim(p, &id, "runner-a").await;
    let current = relaunch(p, &id, "runner-b").await;
    assert!(current.epoch > stale.epoch);

    assert_eq!(
        p.park_execution(&stale, timer_park(), NO_TARGETS)
            .await
            .unwrap(),
        ParkOutcome::Superseded
    );
    assert!(
        !p.complete_instance(
            CompleteInstanceParams::new(&id, InstanceStatus::Completed)
                .owned_by(&stale)
                .with_output(b"stale"),
        )
        .await
        .unwrap(),
        "a superseded execution must not complete its replacement"
    );
    let instance = p.get_instance(&id).await.unwrap().unwrap();
    assert_eq!(instance.status, InstanceStatus::Running);
    assert!(instance.output.is_none());

    assert!(
        p.complete_instance(
            CompleteInstanceParams::new(&id, InstanceStatus::Completed)
                .owned_by(&current)
                .with_output(b"current"),
        )
        .await
        .unwrap()
    );
    let instance = p.get_instance(&id).await.unwrap().unwrap();
    assert_eq!(instance.status, InstanceStatus::Completed);
    assert_eq!(instance.output.as_deref(), Some(&b"current"[..]));
    p.delete_instances_batch(&[id]).await.unwrap();
}

/// A pause or cancel that wins before the execution parks supersedes the
/// park, including its retries.
pub async fn pause_or_cancel_before_park_supersedes_it(p: &dyn Persistence) {
    for kind in [SignalType::Pause, SignalType::Cancel] {
        let id = running(p, "command-race").await;
        let lease = claim(p, &id, "runner-a").await;
        p.insert_signal(&id, kind, b"").await.unwrap();
        let command = p.get_pending_signal(&id).await.unwrap().unwrap();
        assert!(
            p.apply_lifecycle_command(&id, &command.command_id, kind)
                .await
                .unwrap()
                .accepted()
        );
        let before = p.get_instance(&id).await.unwrap().unwrap();
        for _ in 0..2 {
            assert_eq!(
                p.park_execution(&lease, timer_park(), NO_TARGETS)
                    .await
                    .unwrap(),
                ParkOutcome::Superseded,
                "{kind:?} must supersede the park"
            );
        }
        let after = p.get_instance(&id).await.unwrap().unwrap();
        assert_eq!(after.status, before.status);
        assert_eq!(after.sleep_until, before.sleep_until);
        assert_eq!(after.termination_reason, before.termination_reason);
        p.delete_instances_batch(&[id]).await.unwrap();
    }
}

/// A completion presented with the execution's lease applies once; the
/// execution's retry after a lost acknowledgement changes nothing.
pub async fn owned_completion_applies_once(p: &dyn Persistence) {
    let id = running(p, "complete-retry").await;
    let lease = claim(p, &id, "runner-a").await;
    let complete = || {
        CompleteInstanceParams::new(&id, InstanceStatus::Completed)
            .owned_by(&lease)
            .with_output(b"done")
    };
    assert!(p.complete_instance(complete()).await.unwrap());
    assert!(!p.complete_instance(complete()).await.unwrap());
    let instance = p.get_instance(&id).await.unwrap().unwrap();
    assert_eq!(instance.status, InstanceStatus::Completed);
    assert_eq!(instance.output.as_deref(), Some(&b"done"[..]));
    p.delete_instances_batch(&[id]).await.unwrap();
}

/// Every single-backend case in this module.
pub async fn run_all(p: &dyn Persistence) {
    record_checkpoint_first_write_wins(p).await;
    owned_park_retry_is_already_parked(p).await;
    superseded_execution_cannot_park_or_complete(p).await;
    pause_or_cancel_before_park_supersedes_it(p).await;
    owned_completion_applies_once(p).await;
}
