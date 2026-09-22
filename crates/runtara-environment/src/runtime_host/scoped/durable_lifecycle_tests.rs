//! Real prepared child execution with PostgreSQL admission/settlement. This
//! test adapter exercises the host lifecycle bridge; production child IO still
//! needs the fenced persistence methods before this can be enabled there.
use super::*;
use runtara_component_host::isolated_tasks::{TaskCancellation, TaskError, TaskLifecycle};
use runtara_core::persistence::invocations::{AttemptState, InvocationAttempt, InvocationLease};
use tokio::sync::Notify;

struct DatabaseLifecycle {
    persistence: Arc<dyn Persistence>,
    lease: InvocationLease,
    start_id: String,
    started: AtomicBool,
    attempt: Mutex<Option<InvocationAttempt>>,
    admitted: Notify,
    settling: Notify,
    release: Notify,
    hold_admission: bool,
    hold_settlement: bool,
}
impl DatabaseLifecycle {
    async fn resolve(&self) -> Result<InvocationAttempt, TaskError> {
        if let Some(attempt) = self.attempt.lock().unwrap().clone() {
            return Ok(attempt);
        }
        self.persistence
            .invocation_fences()
            .unwrap()
            .begin_invocation_attempt(&self.lease, "parent/child", &self.start_id)
            .await
            .map_err(|_| TaskError::WorkerLost)
    }
}
#[async_trait::async_trait]
impl TaskLifecycle for DatabaseLifecycle {
    async fn admit(&self, _: TaskCancellation) -> Result<Option<InvokeExit>, TaskError> {
        self.started.store(true, Ordering::Release);
        let attempt = self.resolve().await?;
        self.admitted.notify_one();
        // The DB commit happened, but execution can be dropped before this
        // adapter records the reply. Settlement must resolve the same start ID.
        if self.hold_admission {
            std::future::pending::<()>().await;
        }
        *self.attempt.lock().unwrap() = Some(attempt.clone());
        Ok((attempt.state == AttemptState::Cancelled).then_some(InvokeExit::Cancelled))
    }
    async fn settle(
        &self,
        outcome: Result<InvokeExit, TaskError>,
        _: TaskCancellation,
    ) -> Result<InvokeExit, TaskError> {
        self.settling.notify_one();
        if self.hold_settlement {
            self.release.notified().await;
        }
        let fences = self.persistence.invocation_fences().unwrap();
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(error) => {
                fences
                    .revoke_invocation_lease(&self.lease)
                    .await
                    .map_err(|_| TaskError::WorkerLost)?;
                return Err(error);
            }
        };
        if !self.started.load(Ordering::Acquire) {
            return Ok(outcome);
        }
        let attempt = self.resolve().await?;
        if attempt.state == AttemptState::Cancelled {
            return Ok(InvokeExit::Cancelled);
        }
        let settled = fences
            .settle_invocation_attempt(&attempt.fence, None)
            .await
            .map_err(|_| TaskError::WorkerLost)?;
        Ok(if settled.state == AttemptState::Cancelled {
            InvokeExit::Cancelled
        } else {
            outcome
        })
    }
}
struct ManagedScopes {
    scopes: Arc<ScopedInvocationFactory>,
    lifecycle: Arc<DatabaseLifecycle>,
}
impl InvocationScopeFactory for ManagedScopes {
    fn prepare_child(
        &self,
        request: &StartRequest,
    ) -> Result<
        runtara_component_host::ChildInvocationScope,
        runtara_component_host::execution_host::ExecutionError,
    > {
        let mut scope = self.scopes.prepare_child(request)?;
        scope.lifecycle = Some(self.lifecycle.clone());
        Ok(scope)
    }
}
async fn hooks(
    fx: &Fixture,
    hold_admission: bool,
    hold_settlement: bool,
) -> Arc<DatabaseLifecycle> {
    let instance = fx.persistence.get_instance(&fx.id).await.unwrap().unwrap();
    let lease = fx
        .persistence
        .invocation_fences()
        .unwrap()
        .claim_invocation_lease(&instance.tenant_id, &fx.id, "test-launch", None)
        .await
        .unwrap();
    Arc::new(DatabaseLifecycle {
        persistence: fx.persistence.clone(),
        lease,
        start_id: uuid::Uuid::new_v4().to_string(),
        started: AtomicBool::new(false),
        attempt: Mutex::new(None),
        admitted: Notify::new(),
        settling: Notify::new(),
        release: Notify::new(),
        hold_admission,
        hold_settlement,
    })
}
async fn start(
    fx: &Fixture,
    hooks: Arc<DatabaseLifecycle>,
) -> runtara_component_host::isolated_tasks::TaskId {
    let scopes = factory(
        fx,
        settings(
            Instant::now() + Duration::from_secs(10),
            Arc::new(AtomicBool::new(false)),
        ),
    );
    let launcher = PreparedInvocationLauncher::new(
        fx.executor.clone(),
        catalog(fx, None).await,
        Arc::new(ManagedScopes {
            scopes,
            lifecycle: hooks,
        }),
    )
    .unwrap();
    let prepared = launcher.prepare(request("copy")).unwrap();
    fx.tasks
        .spawn_managed(prepared.run, prepared.cleanup, prepared.lifecycle)
        .unwrap()
}
async fn heartbeat_count(fx: &Fixture) -> i64 {
    fx.persistence
        .count_events(
            &fx.id,
            &ListEventsFilter {
                event_type: Some(runtara_core::domain::EventType::Heartbeat),
                ..Default::default()
            },
        )
        .await
        .unwrap()
}
async fn finish(fx: &Fixture, hooks: &DatabaseLifecycle) {
    fx.close().await;
    fx.persistence
        .invocation_fences()
        .unwrap()
        .revoke_invocation_lease(&hooks.lease)
        .await
        .unwrap();
    assert_eq!(fx.status().await, InstanceStatus::Running);
    assert!(
        fx.persistence
            .get_instance(&fx.id)
            .await
            .unwrap()
            .unwrap()
            .output
            .is_none()
    );
}

#[tokio::test]
async fn database_admission_and_settlement_wrap_real_prepared_child_execution() {
    let fx = Fixture::new().await;
    let hooks = hooks(&fx, false, true).await;
    let id = start(&fx, hooks.clone()).await;
    hooks.settling.notified().await;
    assert_eq!(heartbeat_count(&fx).await, 1);
    assert_eq!(hooks.resolve().await.unwrap().state, AttemptState::Active);
    assert!(
        tokio::time::timeout(Duration::from_millis(20), fx.tasks.join(id))
            .await
            .is_err()
    );
    hooks.release.notify_one();
    let result = tokio::time::timeout(Duration::from_secs(5), fx.tasks.join(id))
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(result.outcome(),InvokeExit::Completed(bytes) if *bytes == request("copy").input)
    );
    let stored = fx
        .persistence
        .invocation_fences()
        .unwrap()
        .begin_invocation_attempt(&hooks.lease, "parent/child", &hooks.start_id)
        .await
        .unwrap();
    assert_eq!(stored.state, AttemptState::Settled);
    finish(&fx, &hooks).await;
}

#[tokio::test]
async fn cancellation_during_admission_resolves_uncertain_commit_without_starting_child() {
    let fx = Fixture::new().await;
    let hooks = hooks(&fx, true, false).await;
    let id = start(&fx, hooks.clone()).await;
    hooks.admitted.notified().await;
    assert!(hooks.attempt.lock().unwrap().is_none());
    let original = hooks.resolve().await.unwrap();
    fx.tasks.cancel(id).unwrap();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(5), fx.tasks.join(id))
            .await
            .unwrap()
            .unwrap()
            .outcome(),
        InvokeExit::Cancelled
    ));
    let stored = hooks.resolve().await.unwrap();
    assert_eq!(stored.fence, original.fence);
    // Generic native cancellation closes the attempt without introducing a
    // durable user-cancel tombstone that would break pause/replay semantics.
    assert_eq!(stored.state, AttemptState::Settled);
    assert_eq!(heartbeat_count(&fx).await, 0);
    finish(&fx, &hooks).await;
}

#[tokio::test]
async fn persisted_cancellation_wins_over_computed_child_result_and_replay_admission() {
    let fx = Fixture::new().await;
    let hooks = hooks(&fx, false, true).await;
    let id = start(&fx, hooks.clone()).await;
    hooks.settling.notified().await;
    let fences = fx.persistence.invocation_fences().unwrap();
    let attempt = hooks.resolve().await.unwrap();
    assert_eq!(
        fences
            .cancel_invocation_attempt(&attempt.fence)
            .await
            .unwrap(),
        AttemptState::Cancelled
    );
    hooks.release.notify_one();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(5), fx.tasks.join(id))
            .await
            .unwrap()
            .unwrap()
            .outcome(),
        InvokeExit::Cancelled
    ));
    assert_eq!(heartbeat_count(&fx).await, 1);
    fences.revoke_invocation_lease(&hooks.lease).await.unwrap();
    let lease = fences
        .claim_invocation_lease(
            &hooks.lease.tenant_id,
            &fx.id,
            "replay",
            Some(hooks.lease.epoch),
        )
        .await
        .unwrap();
    let replay = Arc::new(DatabaseLifecycle {
        persistence: fx.persistence.clone(),
        lease,
        start_id: "new-start".into(),
        started: AtomicBool::new(false),
        attempt: Mutex::new(None),
        admitted: Notify::new(),
        settling: Notify::new(),
        release: Notify::new(),
        hold_admission: false,
        hold_settlement: false,
    });
    let replay_id = start(&fx, replay.clone()).await;
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(5), fx.tasks.join(replay_id))
            .await
            .unwrap()
            .unwrap()
            .outcome(),
        InvokeExit::Cancelled
    ));
    assert_eq!(
        heartbeat_count(&fx).await,
        1,
        "replay must not instantiate/invoke the cancelled child"
    );
    finish(&fx, &replay).await;
}
