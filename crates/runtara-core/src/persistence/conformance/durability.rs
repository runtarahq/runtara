//! Durability lifecycle cases shared by every durable backend: first-wins
//! result checkpoints, root execution ownership, and owned lifecycle
//! transitions (see `docs/durability-changes.md`).
use crate::domain::EventType;
use crate::domain::{InstanceStatus, SignalType};
use crate::error::CoreError;
use crate::lifecycle::{ParkReason, ParkRequest, TransitionOutcome};
use crate::persistence::invocations::{InvocationFences, InvocationLease};
use crate::persistence::{
    CheckpointWrite, CompleteInstanceParams, EventRecord, ExecutionWriter, ParkTargets, Persistence,
};

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
        p.record_checkpoint(&id, "step-a", b"first", None)
            .await
            .unwrap(),
        CheckpointWrite::Recorded
    );
    let instance = p.get_instance(&id).await.unwrap().unwrap();
    assert_eq!(instance.checkpoint_id.as_deref(), Some("step-a"));

    p.record_checkpoint(&id, "step-b", b"other", None)
        .await
        .unwrap();
    assert_eq!(
        p.record_checkpoint(&id, "step-a", b"second", None)
            .await
            .unwrap(),
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
                let write = p
                    .record_checkpoint(&id, "race", &bytes, None)
                    .await
                    .unwrap();
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
        TransitionOutcome::Applied
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
        TransitionOutcome::AlreadyApplied
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
        TransitionOutcome::Superseded
    );
    assert_eq!(
        p.complete_execution(
            &stale,
            CompleteInstanceParams::new(&id, InstanceStatus::Completed).with_output(b"stale"),
            Some(&terminal_event(&id, EventType::Completed)),
        )
        .await
        .unwrap(),
        TransitionOutcome::Superseded,
        "a superseded execution must not complete its replacement"
    );
    assert_eq!(terminal_events(p, &id).await, 0, "nor append its event");
    let instance = p.get_instance(&id).await.unwrap().unwrap();
    assert_eq!(instance.status, InstanceStatus::Running);
    assert!(instance.output.is_none());

    assert_eq!(
        p.complete_execution(
            &current,
            CompleteInstanceParams::new(&id, InstanceStatus::Completed).with_output(b"current"),
            Some(&terminal_event(&id, EventType::Completed)),
        )
        .await
        .unwrap(),
        TransitionOutcome::Applied
    );
    assert_eq!(terminal_events(p, &id).await, 1);
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
                TransitionOutcome::Superseded,
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

fn terminal_event(id: &str, event_type: EventType) -> EventRecord {
    EventRecord {
        id: None,
        instance_id: id.to_string(),
        event_type,
        checkpoint_id: None,
        payload: None,
        created_at: chrono::Utc::now(),
        subtype: None,
    }
}

async fn terminal_events(p: &dyn Persistence, id: &str) -> i64 {
    let mut total = 0;
    for event_type in [
        EventType::Completed,
        EventType::Failed,
        EventType::Suspended,
    ] {
        total += p
            .count_events(
                id,
                &crate::persistence::ListEventsFilter {
                    event_type: Some(event_type),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
    }
    total
}

/// An execution's completion commits its terminal event with the
/// transition, once: a retry after a lost acknowledgement is recognised and
/// writes nothing, not even a second event.
pub async fn owned_completion_applies_once(p: &dyn Persistence) {
    for status in [
        InstanceStatus::Completed,
        InstanceStatus::Failed,
        InstanceStatus::Suspended,
    ] {
        let id = running(p, "complete-retry").await;
        let lease = claim(p, &id, "runner-a").await;
        let event_type = match status {
            InstanceStatus::Completed => EventType::Completed,
            InstanceStatus::Failed => EventType::Failed,
            _ => EventType::Suspended,
        };
        let terminal = terminal_event(&id, event_type);
        let complete = || {
            p.complete_execution(
                &lease,
                CompleteInstanceParams::new(&id, status).with_output(b"done"),
                Some(&terminal),
            )
        };
        assert_eq!(
            complete().await.unwrap(),
            TransitionOutcome::Applied,
            "{status:?}"
        );
        assert_eq!(
            complete().await.unwrap(),
            TransitionOutcome::AlreadyApplied,
            "{status:?}"
        );
        let instance = p.get_instance(&id).await.unwrap().unwrap();
        assert_eq!(instance.status, status);
        assert_eq!(instance.output.as_deref(), Some(&b"done"[..]));
        assert_eq!(
            terminal_events(p, &id).await,
            1,
            "{status:?}: the event commits with the transition, once"
        );
        p.delete_instances_batch(&[id]).await.unwrap();
    }
}

/// A transition is recognised as already applied only when it is the one
/// this execution committed: a different status, a park, or a completion
/// after a park is superseded and writes nothing.
pub async fn only_the_committed_transition_is_already_applied(p: &dyn Persistence) {
    let id = running(p, "other-transition").await;
    let lease = claim(p, &id, "runner-a").await;
    assert_eq!(
        p.complete_execution(
            &lease,
            CompleteInstanceParams::new(&id, InstanceStatus::Completed).with_output(b"done"),
            None,
        )
        .await
        .unwrap(),
        TransitionOutcome::Applied
    );
    assert_eq!(
        p.complete_execution(
            &lease,
            CompleteInstanceParams::new(&id, InstanceStatus::Failed).with_error("late"),
            Some(&terminal_event(&id, EventType::Failed)),
        )
        .await
        .unwrap(),
        TransitionOutcome::Superseded
    );
    assert_eq!(
        p.park_execution(&lease, timer_park(), NO_TARGETS)
            .await
            .unwrap(),
        TransitionOutcome::Superseded
    );
    let instance = p.get_instance(&id).await.unwrap().unwrap();
    assert_eq!(instance.status, InstanceStatus::Completed);
    assert_eq!(terminal_events(p, &id).await, 0);

    // A parked execution's later completion is not its committed park.
    let parked = running(p, "park-then-complete").await;
    let lease = claim(p, &parked, "runner-a").await;
    assert_eq!(
        p.park_execution(&lease, timer_park(), NO_TARGETS)
            .await
            .unwrap(),
        TransitionOutcome::Applied
    );
    assert_eq!(
        p.complete_execution(
            &lease,
            CompleteInstanceParams::new(&parked, InstanceStatus::Suspended),
            Some(&terminal_event(&parked, EventType::Suspended)),
        )
        .await
        .unwrap(),
        TransitionOutcome::Superseded
    );
    assert_eq!(terminal_events(p, &parked).await, 0);
    p.delete_instances_batch(&[id, parked]).await.unwrap();
}

/// A pause or cancel that wins first supersedes the execution's completion.
pub async fn pause_or_cancel_before_completion_supersedes_it(p: &dyn Persistence) {
    for kind in [SignalType::Pause, SignalType::Cancel] {
        let id = running(p, "complete-race").await;
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
        assert_eq!(
            p.complete_execution(
                &lease,
                CompleteInstanceParams::new(&id, InstanceStatus::Completed).with_output(b"late"),
                Some(&terminal_event(&id, EventType::Completed)),
            )
            .await
            .unwrap(),
            TransitionOutcome::Superseded,
            "{kind:?}"
        );
        let after = p.get_instance(&id).await.unwrap().unwrap();
        assert_eq!(after.status, before.status);
        assert!(after.output.is_none());
        p.delete_instances_batch(&[id]).await.unwrap();
    }
}

fn event(id: &str) -> EventRecord {
    EventRecord {
        id: None,
        instance_id: id.to_string(),
        event_type: EventType::Custom,
        checkpoint_id: None,
        payload: Some(b"{}".to_vec()),
        created_at: chrono::Utc::now(),
        subtype: Some("durability".into()),
    }
}

fn superseded<T: std::fmt::Debug>(result: Result<T, CoreError>, what: &str) {
    assert!(
        matches!(result, Err(CoreError::Superseded { .. })),
        "{what} must be refused as superseded, got {result:?}"
    );
}

/// Every guest write a backend offers, by `owner`, against one instance.
async fn guest_writes(
    p: &dyn Persistence,
    id: &str,
    owner: ExecutionWriter<'_>,
    tag: &str,
) -> Vec<(&'static str, Result<(), CoreError>)> {
    let mut writes = vec![
        (
            "record_checkpoint",
            p.record_checkpoint(id, &format!("{tag}-result"), b"r", owner)
                .await
                .map(|_| ()),
        ),
        (
            "save_sleep_checkpoint",
            p.save_sleep_checkpoint(id, &format!("{tag}-sleep"), b"s", owner)
                .await,
        ),
        (
            "append_execution_event",
            p.append_execution_event(&event(id), owner).await,
        ),
        (
            "record_retry_attempt",
            p.record_retry_attempt(id, &format!("{tag}-retry"), 1, Some("boom"), owner)
                .await,
        ),
    ];
    if let Some(continuations) = p.agent_continuations() {
        let op = format!("{:02x}", tag.len()).repeat(32);
        writes.push((
            "continuation put",
            continuations.put(id, &op, 1, b"c", owner).await,
        ));
    }
    writes
}

/// After a replacement execution owns the root, every guest write of the
/// superseded execution is refused and leaves the replacement's state
/// alone; the replacement's own writes apply.
pub async fn superseded_execution_cannot_write_guest_state(p: &dyn Persistence) {
    let id = running(p, "stale-writes").await;
    let stale = claim(p, &id, "runner-a").await;
    let current = relaunch(p, &id, "runner-b").await;
    let events = p.count_events(&id, &Default::default()).await.unwrap();
    let checkpoints = p.count_checkpoints(&id, None, None, None).await.unwrap();

    for (what, result) in guest_writes(p, &id, Some(&stale), "stale").await {
        superseded(result, what);
    }
    assert_eq!(
        p.count_events(&id, &Default::default()).await.unwrap(),
        events
    );
    assert_eq!(
        p.count_checkpoints(&id, None, None, None).await.unwrap(),
        checkpoints,
        "a superseded execution must not write checkpoints"
    );
    let instance = p.get_instance(&id).await.unwrap().unwrap();
    assert_eq!(instance.status, InstanceStatus::Running);
    assert_ne!(instance.checkpoint_id.as_deref(), Some("stale-result"));
    assert_ne!(instance.checkpoint_id.as_deref(), Some("stale-sleep"));

    for (what, result) in guest_writes(p, &id, Some(&current), "current").await {
        assert!(result.is_ok(), "{what} by the owner must apply: {result:?}");
    }
    assert!(p.count_events(&id, &Default::default()).await.unwrap() > events);

    // Once the owner leaves `running` its lease is spent too.
    assert_eq!(
        p.complete_execution(
            &current,
            CompleteInstanceParams::new(&id, InstanceStatus::Completed),
            None,
        )
        .await
        .unwrap(),
        TransitionOutcome::Applied
    );
    for (what, result) in guest_writes(p, &id, Some(&current), "late").await {
        assert!(result.is_err(), "{what} after completion must be refused");
    }
    p.delete_instances_batch(&[id]).await.unwrap();
}

/// A writer outside any launched execution (no lease) is refused while an
/// execution owns the root, and admitted when none does.
pub async fn unowned_writes_yield_to_an_owning_execution(p: &dyn Persistence) {
    let id = running(p, "unowned").await;
    for (what, result) in guest_writes(p, &id, None, "free").await {
        assert!(
            result.is_ok(),
            "{what} without any lease must apply: {result:?}"
        );
    }
    let _owner = claim(p, &id, "runner-a").await;
    for (what, result) in guest_writes(p, &id, None, "owned").await {
        superseded(result, what);
    }
    p.delete_instances_batch(&[id]).await.unwrap();
}

/// The fenced child lookup reads without writing and stays fenced: a
/// superseded attempt cannot read through it either.
pub async fn fenced_child_lookup_reads_only_under_its_fence(p: &dyn Persistence) {
    use crate::persistence::invocations::InvocationCheckpoint;
    let id = running(p, "child-lookup").await;
    let lease = claim(p, &id, "runner-a").await;
    let attempt = fences(p)
        .begin_invocation_attempt(&lease, "child", "start")
        .await
        .unwrap()
        .fence;
    assert_eq!(
        fences(p)
            .invocation_checkpoint_lookup(&attempt, "child::step")
            .await
            .unwrap(),
        None
    );
    assert!(
        p.load_checkpoint(&id, "child::step")
            .await
            .unwrap()
            .is_none(),
        "a lookup must not write"
    );
    fences(p)
        .invocation_checkpoint(
            &attempt,
            &InvocationCheckpoint {
                checkpoint_id: "child::step".into(),
                state: b"child".to_vec(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        fences(p)
            .invocation_checkpoint_lookup(&attempt, "child::step")
            .await
            .unwrap(),
        Some(b"child".to_vec())
    );
    relaunch(p, &id, "runner-b").await;
    assert!(
        fences(p)
            .invocation_checkpoint_lookup(&attempt, "child::step")
            .await
            .is_err(),
        "a superseded attempt cannot read through its fence"
    );
    p.delete_instances_batch(&[id]).await.unwrap();
}

/// Every single-backend case in this module.
pub async fn run_all(p: &dyn Persistence) {
    record_checkpoint_first_write_wins(p).await;
    owned_park_retry_is_already_parked(p).await;
    superseded_execution_cannot_park_or_complete(p).await;
    pause_or_cancel_before_park_supersedes_it(p).await;
    owned_completion_applies_once(p).await;
    only_the_committed_transition_is_already_applied(p).await;
    pause_or_cancel_before_completion_supersedes_it(p).await;
    superseded_execution_cannot_write_guest_state(p).await;
    unowned_writes_yield_to_an_owning_execution(p).await;
    fenced_child_lookup_reads_only_under_its_fence(p).await;
}
