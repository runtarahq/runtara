//! Durable instance-wait cases shared by every backend with instance waits.
use std::future::Future;
use std::pin::Pin;

use chrono::{DateTime, Duration, Utc};

use crate::domain::{InstanceStatus, SignalType, WakeReason};
use crate::lifecycle::{Decision, ParkReason, ParkRequest};
use crate::persistence::waits::*;
use crate::persistence::{
    CompleteInstanceParams, ExternalOutcome, ExternalOutcomeKind, ParentLink, ParkTargets,
    Persistence, PublishOutcome,
};

/// Simulates a crash after a target finished and before the waiter's wake
/// was stamped: clears the waiter's wake, its park's wake claim and every
/// nudge on its waits. Each backend provides it through its own storage.
pub type LoseWake<'a> =
    &'a (dyn Fn(String) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync);

fn waits(p: &dyn Persistence) -> &dyn InstanceWaits {
    p.instance_waits().expect("instance waits required")
}

struct Family {
    tenant: String,
    waiter: String,
}

impl Family {
    async fn new(p: &dyn Persistence, name: &str) -> Self {
        let tenant = format!("waits-{name}-{}", uuid::Uuid::new_v4());
        let waiter = format!("{tenant}-waiter");
        p.register_instance(&waiter, &tenant).await.unwrap();
        p.update_instance_status(&waiter, InstanceStatus::Running, None)
            .await
            .unwrap();
        Self { tenant, waiter }
    }

    async fn child(&self, p: &dyn Persistence, name: &str) -> String {
        let id = format!("{}-{name}", self.tenant);
        assert!(
            p.try_register_child_instance(
                &id,
                &self.tenant,
                None,
                None,
                &ParentLink {
                    parent_instance_id: self.waiter.clone(),
                    parent_close_policy: "cancel".into(),
                    admitted_at: Utc::now(),
                },
            )
            .await
            .unwrap()
        );
        p.update_instance_status(&id, InstanceStatus::Running, None)
            .await
            .unwrap();
        id
    }

    fn spec(&self, targets: &[&String], mode: WaitMode) -> WaitSpec {
        WaitSpec::new(targets.iter().map(|t| (*t).clone()), mode, None)
    }

    async fn park(&self, p: &dyn Persistence, waits: &[&str]) -> Decision {
        let waits: Vec<String> = waits.iter().map(|w| (*w).to_owned()).collect();
        p.park_instance_on_targets(
            &self.waiter,
            ParkRequest {
                reason: ParkReason::Instances,
                deadline: Some(Utc::now() + Duration::hours(1)),
            },
            ParkTargets {
                signal_ids: &[],
                wait_ids: &waits,
            },
        )
        .await
        .unwrap()
    }

    async fn woken(&self, p: &dyn Persistence) -> bool {
        let row = p.get_instance(&self.waiter).await.unwrap().unwrap();
        row.wake_reason == Some(WakeReason::InstancesTerminal)
            && row.sleep_until.is_some_and(|at| at <= Utc::now())
    }

    async fn relaunch(&self, p: &dyn Persistence) {
        p.update_instance_status(&self.waiter, InstanceStatus::Running, Some(Utc::now()))
            .await
            .unwrap();
    }

    async fn cleanup(&self, p: &dyn Persistence, others: &[&String]) {
        let mut ids = vec![self.waiter.clone()];
        ids.extend(others.iter().map(|id| (*id).clone()));
        p.delete_instances_batch(&ids).await.unwrap();
    }
}

async fn finish(p: &dyn Persistence, id: &str, status: InstanceStatus) {
    p.complete_instance(CompleteInstanceParams::new(id, status).with_output(b"{}"))
        .await
        .unwrap();
}

fn ids(targets: &[WaitTarget]) -> Vec<&str> {
    targets.iter().map(|t| t.instance_id.as_str()).collect()
}

/// The rule: pending, then satisfied with the finished selection; `any` keeps
/// its selection; an empty wait resolves at once; never-launched outcomes
/// count as finished; unknown and foreign targets do not.
pub async fn resolution(p: &dyn Persistence) {
    let family = Family::new(p, "resolution").await;
    let (a, b, c) = (
        family.child(p, "a").await,
        family.child(p, "b").await,
        family.child(p, "c").await,
    );
    let w = waits(p);
    let all = family.spec(&[&b, &a], WaitMode::All);
    let view = w
        .register_or_evaluate(&family.tenant, &family.waiter, "op-all", &all)
        .await
        .unwrap();
    assert_eq!(view.record.state, WaitState::Pending);
    assert_eq!(view.record.targets, [a.clone(), b.clone()], "sorted");
    assert!(view.finished.is_empty());
    assert_eq!(ids(&view.remaining), [a.as_str(), b.as_str()]);

    finish(p, &b, InstanceStatus::Failed).await;
    let view = w
        .poll_wait(&family.tenant, &family.waiter, "op-all")
        .await
        .unwrap();
    assert_eq!(view.record.state, WaitState::Pending);
    assert_eq!(ids(&view.finished), [b.as_str()]);
    assert_eq!(ids(&view.remaining), [a.as_str()]);

    finish(p, &a, InstanceStatus::Completed).await;
    let view = w
        .poll_wait(&family.tenant, &family.waiter, "op-all")
        .await
        .unwrap();
    assert_eq!(view.resolution(), Some(WaitResolution::Satisfied));
    assert_eq!(
        ids(&view.finished),
        [b.as_str(), a.as_str()],
        "finish order"
    );
    assert!(view.remaining.is_empty());

    // `any` selects every target finished at resolution and keeps it.
    let any = family.spec(&[&a, &b, &c], WaitMode::Any);
    let view = w
        .register_or_evaluate(&family.tenant, &family.waiter, "op-any", &any)
        .await
        .unwrap();
    assert_eq!(view.resolution(), Some(WaitResolution::Satisfied));
    assert_eq!(ids(&view.finished), [b.as_str(), a.as_str()]);
    assert_eq!(ids(&view.remaining), [c.as_str()]);
    finish(p, &c, InstanceStatus::Cancelled).await;
    let replay = w
        .poll_wait(&family.tenant, &family.waiter, "op-any")
        .await
        .unwrap();
    assert_eq!(ids(&replay.finished), [b.as_str(), a.as_str()]);
    assert_eq!(ids(&replay.remaining), [c.as_str()]);
    assert!(
        replay.remaining[0].state.is_finished(),
        "the remaining target reports its current state"
    );

    // No targets: empty at once.
    let empty = WaitSpec::new(Vec::<String>::new(), WaitMode::All, None);
    let view = w
        .register_or_evaluate(&family.tenant, &family.waiter, "op-empty", &empty)
        .await
        .unwrap();
    assert_eq!(view.resolution(), Some(WaitResolution::Empty));

    // A never-launched outcome finishes its target; an unknown id does not.
    let unlaunched = format!("{}-unlaunched", family.tenant);
    let unknown = format!("{}-unknown", family.tenant);
    let foreign = Family::new(p, "foreign").await;
    let view = w
        .register_or_evaluate(
            &family.tenant,
            &family.waiter,
            "op-outcome",
            &WaitSpec::new(
                [unlaunched.clone(), unknown.clone(), foreign.waiter.clone()],
                WaitMode::Any,
                None,
            ),
        )
        .await
        .unwrap();
    assert_eq!(view.record.state, WaitState::Pending);
    assert!(
        view.remaining
            .iter()
            .all(|t| t.state == TargetState::Unknown),
        "unknown and foreign targets are unknown: {:?}",
        view.remaining
    );
    assert_eq!(
        p.publish_external_outcome(&ExternalOutcome {
            instance_id: unlaunched.clone(),
            tenant_id: family.tenant.clone(),
            parent_instance_id: family.waiter.clone(),
            outcome: ExternalOutcomeKind::NotStarted,
            reason: Some("expired".into()),
            admitted_at: Utc::now(),
            workflow_id: None,
            workflow_version: None,
            run_label: None,
        })
        .await
        .unwrap(),
        PublishOutcome::Published
    );
    let view = w
        .poll_wait(&family.tenant, &family.waiter, "op-outcome")
        .await
        .unwrap();
    assert_eq!(view.resolution(), Some(WaitResolution::Satisfied));
    assert_eq!(ids(&view.finished), [unlaunched.as_str()]);
    assert!(matches!(
        view.finished[0].state,
        TargetState::Outcome {
            outcome: ExternalOutcomeKind::NotStarted,
            ..
        }
    ));

    // Another tenant cannot reach the waiter's waits.
    assert_eq!(
        w.poll_wait(&foreign.tenant, &family.waiter, "op-all")
            .await
            .unwrap_err(),
        WaitError::NotFound
    );
    family.cleanup(p, &[&a, &b, &c]).await;
    foreign.cleanup(p, &[]).await;
}

/// Replays return the registered wait (first deadline wins); another target
/// set or mode conflicts; malformed and finished waiters are refused.
pub async fn replay(p: &dyn Persistence) {
    let family = Family::new(p, "replay").await;
    let (a, b) = (family.child(p, "a").await, family.child(p, "b").await);
    let w = waits(p);
    let first_deadline = Utc::now() + Duration::hours(2);
    let first = WaitSpec::new([a.clone(), b.clone()], WaitMode::All, Some(first_deadline));
    let registered = w
        .register_or_evaluate(&family.tenant, &family.waiter, "op", &first)
        .await
        .unwrap();
    assert_eq!(
        registered.record.deadline,
        Some(truncate_ms(first_deadline))
    );
    // Same targets (reordered, duplicated), a later deadline: the same wait.
    let again = WaitSpec::new(
        [b.clone(), a.clone(), a.clone()],
        WaitMode::All,
        Some(Utc::now() - Duration::hours(1)),
    );
    let replayed = w
        .register_or_evaluate(&family.tenant, &family.waiter, "op", &again)
        .await
        .unwrap();
    assert_eq!(
        replayed.record, registered.record,
        "the first deadline wins"
    );
    assert_eq!(replayed.record.state, WaitState::Pending);
    for conflicting in [
        WaitSpec::new([a.clone()], WaitMode::All, None),
        WaitSpec::new([a.clone(), b.clone()], WaitMode::Any, None),
    ] {
        assert_eq!(
            w.register_or_evaluate(&family.tenant, &family.waiter, "op", &conflicting)
                .await
                .unwrap_err(),
            WaitError::Conflict
        );
    }
    let too_many = WaitSpec::new(
        (0..=MAX_WAIT_TARGETS).map(|i| format!("{}-{i}", family.tenant)),
        WaitMode::All,
        None,
    );
    assert_eq!(
        w.register_or_evaluate(&family.tenant, &family.waiter, "big", &too_many)
            .await
            .unwrap_err(),
        WaitError::TooLarge
    );
    assert!(matches!(
        w.register_or_evaluate(
            &family.tenant,
            &family.waiter,
            "self",
            &WaitSpec::new([family.waiter.clone()], WaitMode::Any, None),
        )
        .await,
        Err(WaitError::Invalid(_))
    ));
    assert_eq!(
        w.register_or_evaluate(&family.tenant, "no-such-waiter", "op", &first)
            .await
            .unwrap_err(),
        WaitError::NotFound
    );
    finish(p, &family.waiter, InstanceStatus::Completed).await;
    assert_eq!(
        w.register_or_evaluate(&family.tenant, &family.waiter, "late", &first)
            .await
            .unwrap_err(),
        WaitError::Inactive
    );
    family.cleanup(p, &[&a, &b]).await;
}

/// A passed deadline resolves `deadline` with the targets that did finish;
/// the condition wins over a passed deadline.
pub async fn deadline(p: &dyn Persistence) {
    let family = Family::new(p, "deadline").await;
    let (a, b) = (family.child(p, "a").await, family.child(p, "b").await);
    let w = waits(p);
    finish(p, &a, InstanceStatus::Completed).await;
    let past = Utc::now() - Duration::seconds(1);
    let view = w
        .register_or_evaluate(
            &family.tenant,
            &family.waiter,
            "late",
            &WaitSpec::new([a.clone(), b.clone()], WaitMode::All, Some(past)),
        )
        .await
        .unwrap();
    assert_eq!(view.resolution(), Some(WaitResolution::Deadline));
    assert_eq!(ids(&view.finished), [a.as_str()]);
    assert_eq!(ids(&view.remaining), [b.as_str()]);
    // Resolved once: finishing later changes nothing.
    finish(p, &b, InstanceStatus::Completed).await;
    let view = w
        .poll_wait(&family.tenant, &family.waiter, "late")
        .await
        .unwrap();
    assert_eq!(view.resolution(), Some(WaitResolution::Deadline));
    assert_eq!(ids(&view.finished), [a.as_str()]);

    let satisfied = w
        .register_or_evaluate(
            &family.tenant,
            &family.waiter,
            "both",
            &WaitSpec::new([a.clone(), b.clone()], WaitMode::All, Some(past)),
        )
        .await
        .unwrap();
    assert_eq!(satisfied.resolution(), Some(WaitResolution::Satisfied));
    family.cleanup(p, &[&a, &b]).await;
}

/// Close, poll-after-close, re-registration of a closed wait, deletion of a
/// resolved one, and deletion with the waiter.
pub async fn lifecycle(p: &dyn Persistence) {
    let family = Family::new(p, "lifecycle").await;
    let a = family.child(p, "a").await;
    let w = waits(p);
    let spec = family.spec(&[&a], WaitMode::All);
    assert_eq!(
        w.poll_wait(&family.tenant, &family.waiter, "op")
            .await
            .unwrap_err(),
        WaitError::NotFound
    );
    assert!(
        !w.close_wait(&family.tenant, &family.waiter, "op")
            .await
            .unwrap()
    );
    w.register_or_evaluate(&family.tenant, &family.waiter, "op", &spec)
        .await
        .unwrap();
    assert!(
        !w.delete_resolved_wait(&family.tenant, &family.waiter, "op")
            .await
            .unwrap(),
        "a pending wait is not deleted"
    );
    assert!(
        w.close_wait(&family.tenant, &family.waiter, "op")
            .await
            .unwrap()
    );
    assert!(
        !w.close_wait(&family.tenant, &family.waiter, "op")
            .await
            .unwrap()
    );
    assert_eq!(
        w.poll_wait(&family.tenant, &family.waiter, "op")
            .await
            .unwrap_err(),
        WaitError::Closed
    );
    // A retried operation registers afresh over its closed wait.
    let b = family.child(p, "b").await;
    let fresh = w
        .register_or_evaluate(
            &family.tenant,
            &family.waiter,
            "op",
            &family.spec(&[&b], WaitMode::Any),
        )
        .await
        .unwrap();
    assert_eq!(fresh.record.targets, std::slice::from_ref(&b));
    assert_eq!(fresh.record.state, WaitState::Pending);
    // A finish of the closed wait's target does not touch the fresh one.
    finish(p, &a, InstanceStatus::Completed).await;
    assert_eq!(
        w.poll_wait(&family.tenant, &family.waiter, "op")
            .await
            .unwrap()
            .record
            .state,
        WaitState::Pending
    );
    finish(p, &b, InstanceStatus::Completed).await;
    assert!(
        w.poll_wait(&family.tenant, &family.waiter, "op")
            .await
            .unwrap()
            .resolution()
            .is_some()
    );
    assert!(
        w.delete_resolved_wait(&family.tenant, &family.waiter, "op")
            .await
            .unwrap()
    );
    assert_eq!(
        w.poll_wait(&family.tenant, &family.waiter, "op")
            .await
            .unwrap_err(),
        WaitError::NotFound
    );
    // Waits go with their waiter.
    w.register_or_evaluate(&family.tenant, &family.waiter, "kept", &spec)
        .await
        .unwrap();
    family.cleanup(p, &[&a, &b]).await;
    p.register_instance(&family.waiter, &family.tenant)
        .await
        .unwrap();
    assert_eq!(
        w.poll_wait(&family.tenant, &family.waiter, "kept")
            .await
            .unwrap_err(),
        WaitError::NotFound
    );
    p.delete_instances_batch(std::slice::from_ref(&family.waiter))
        .await
        .unwrap();
}

/// Parking: a finish wakes the parked waiter once; a target that finished
/// before the park self-wakes it; `all` waits for every target; a missing
/// wait wakes at once; a paused waiter is never woken.
pub async fn park(p: &dyn Persistence) {
    let w = waits(p);

    // Woken in the finishing writer, once.
    let family = Family::new(p, "park").await;
    let a = family.child(p, "a").await;
    w.register_or_evaluate(
        &family.tenant,
        &family.waiter,
        "op",
        &family.spec(&[&a], WaitMode::All),
    )
    .await
    .unwrap();
    assert!(matches!(
        family.park(p, &["op"]).await,
        Decision::Applied(_)
    ));
    let parked = p.get_instance(&family.waiter).await.unwrap().unwrap();
    assert_eq!(
        parked.termination_reason.as_deref(),
        Some("waiting_instances")
    );
    assert_eq!(parked.wake_reason, Some(WakeReason::Timer), "the deadline");
    assert!(!family.woken(p).await);
    finish(p, &a, InstanceStatus::Completed).await;
    assert!(family.woken(p).await, "the finish wakes the parked waiter");
    let stamped = p
        .get_instance(&family.waiter)
        .await
        .unwrap()
        .unwrap()
        .sleep_until;
    for _ in 0..FULL_RECONCILE_EVERY {
        w.reconcile_wait_wakes(1000).await.unwrap();
    }
    assert_eq!(
        p.get_instance(&family.waiter)
            .await
            .unwrap()
            .unwrap()
            .sleep_until,
        stamped,
        "woken once"
    );
    family.cleanup(p, &[&a]).await;

    // A target finished between registration and park: the park self-wakes.
    let family = Family::new(p, "raced").await;
    let a = family.child(p, "a").await;
    w.register_or_evaluate(
        &family.tenant,
        &family.waiter,
        "op",
        &family.spec(&[&a], WaitMode::Any),
    )
    .await
    .unwrap();
    finish(p, &a, InstanceStatus::Failed).await;
    family.park(p, &["op"]).await;
    assert!(family.woken(p).await);
    assert_eq!(
        w.poll_wait(&family.tenant, &family.waiter, "op")
            .await
            .unwrap()
            .resolution(),
        Some(WaitResolution::Satisfied),
        "the park persisted the resolution"
    );
    family.cleanup(p, &[&a]).await;

    // `all`: one finish is not enough.
    let family = Family::new(p, "all").await;
    let (a, b) = (family.child(p, "a").await, family.child(p, "b").await);
    w.register_or_evaluate(
        &family.tenant,
        &family.waiter,
        "op",
        &family.spec(&[&a, &b], WaitMode::All),
    )
    .await
    .unwrap();
    family.park(p, &["op"]).await;
    finish(p, &a, InstanceStatus::Completed).await;
    assert!(!family.woken(p).await);
    finish(p, &b, InstanceStatus::Completed).await;
    assert!(family.woken(p).await);
    // Relaunched: a later finish of another wait's target does not re-wake.
    family.relaunch(p).await;
    family.cleanup(p, &[&a, &b]).await;

    // A park on a wait that does not exist wakes at once.
    let family = Family::new(p, "missing").await;
    family.park(p, &["never-registered"]).await;
    assert!(family.woken(p).await);
    family.cleanup(p, &[]).await;

    // Paused waiters are never woken, by a finish or by the reconciler.
    let family = Family::new(p, "paused").await;
    let a = family.child(p, "a").await;
    w.register_or_evaluate(
        &family.tenant,
        &family.waiter,
        "op",
        &family.spec(&[&a], WaitMode::Any),
    )
    .await
    .unwrap();
    family.park(p, &["op"]).await;
    p.insert_signal(&family.waiter, SignalType::Pause, b"")
        .await
        .unwrap();
    assert_eq!(
        p.pause_suspended_instances(Some(&family.waiter), 1)
            .await
            .unwrap()
            .len(),
        1,
        "pause_parked covers a run waiting on instances"
    );
    finish(p, &a, InstanceStatus::Completed).await;
    for _ in 0..FULL_RECONCILE_EVERY {
        w.reconcile_wait_wakes(1000).await.unwrap();
    }
    let paused = p.get_instance(&family.waiter).await.unwrap().unwrap();
    assert!(paused.sleep_until.is_none() && paused.wake_reason.is_none());
    assert!(paused.termination_reason.is_none());
    family.cleanup(p, &[&a]).await;
}

/// The reconciler wakes parked waiters whose wake was lost, rotating its full
/// pass so waits that stay pending cannot starve the others, and wakes each
/// once.
pub async fn reconciler_rotation(p: &dyn Persistence, lose_wake: LoseWake<'_>) {
    let w = waits(p);
    let mut pending = Vec::new();
    let mut satisfied = Vec::new();
    // Waiters that stay pending sort first, so a pass without rotation would
    // keep selecting them.
    for i in 0..3 {
        let family = Family::new(p, &format!("a{i}")).await;
        let child = family.child(p, "child").await;
        w.register_or_evaluate(
            &family.tenant,
            &family.waiter,
            "op",
            &family.spec(&[&child], WaitMode::All),
        )
        .await
        .unwrap();
        family.park(p, &["op"]).await;
        pending.push((family, child));
    }
    for i in 0..3 {
        let family = Family::new(p, &format!("z{i}")).await;
        let child = family.child(p, "child").await;
        w.register_or_evaluate(
            &family.tenant,
            &family.waiter,
            "op",
            &family.spec(&[&child], WaitMode::All),
        )
        .await
        .unwrap();
        family.park(p, &["op"]).await;
        finish(p, &child, InstanceStatus::Completed).await;
        assert!(family.woken(p).await);
        lose_wake(family.waiter.clone()).await;
        assert!(!family.woken(p).await, "the wake is lost");
        satisfied.push((family, child));
    }
    let mut woken_total = 0;
    for _ in 0..(FULL_RECONCILE_EVERY * 40) {
        woken_total += w.reconcile_wait_wakes(2).await.unwrap();
        let mut all = true;
        for (family, _) in &satisfied {
            all &= family.woken(p).await;
        }
        if all {
            break;
        }
    }
    for (family, _) in &satisfied {
        assert!(family.woken(p).await, "{} was never woken", family.waiter);
    }
    assert!(woken_total >= satisfied.len() as u64);
    for (family, _) in &pending {
        assert!(!family.woken(p).await, "a pending wait never wakes");
        let row = p.get_instance(&family.waiter).await.unwrap().unwrap();
        assert_eq!(row.wake_reason, Some(WakeReason::Timer));
    }
    // Woken once: more passes stamp nothing new for them.
    let stamps: Vec<Option<DateTime<Utc>>> = {
        let mut stamps = Vec::new();
        for (family, _) in &satisfied {
            stamps.push(
                p.get_instance(&family.waiter)
                    .await
                    .unwrap()
                    .unwrap()
                    .sleep_until,
            );
        }
        stamps
    };
    for _ in 0..FULL_RECONCILE_EVERY {
        w.reconcile_wait_wakes(1000).await.unwrap();
    }
    for ((family, _), stamp) in satisfied.iter().zip(stamps) {
        assert_eq!(
            p.get_instance(&family.waiter)
                .await
                .unwrap()
                .unwrap()
                .sleep_until,
            stamp
        );
    }
    for (family, child) in pending.iter().chain(&satisfied) {
        family.cleanup(p, &[child]).await;
    }
}

/// Every case but the rotation, which needs a backend hook.
pub async fn run_all(p: &dyn Persistence) {
    resolution(p).await;
    replay(p).await;
    deadline(p).await;
    lifecycle(p).await;
    park(p).await;
}
