use super::*;
use runtara_component_host::isolated_tasks::{IsolatedTasks, TaskId};
use tokio::sync::Notify;

struct Fixture {
    admission: Arc<InvocationAdmission>,
    tasks: Arc<IsolatedTasks>,
}
impl Fixture {
    async fn new(timeout: Duration) -> Self {
        let (persistence, id) = crate::test_support::running_instance("admission").await;
        let root = persistence.get_instance(&id).await.unwrap().unwrap();
        let lease = persistence
            .invocation_fences()
            .unwrap()
            .claim_invocation_lease(&root.tenant_id, &id, "test-launch", None)
            .await
            .unwrap();
        let admission = Arc::new(InvocationAdmission::new(
            persistence,
            lease,
            "parent/child".into(),
            timeout,
        ));
        let engine = runtara_component_host::build_engine(&runtara_component_host::EngineConfig {
            cache_dir: None,
            ..Default::default()
        })
        .unwrap();
        Self {
            admission,
            tasks: Arc::new(IsolatedTasks::new(engine, 4, 1024).unwrap()),
        }
    }
    async fn count(&self) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM invocation_attempts WHERE instance_id=$1")
            .bind(&self.admission.lease.instance_id)
            .fetch_one(&crate::test_support::pool().await)
            .await
            .unwrap()
    }
    async fn join(
        &self,
        id: TaskId,
    ) -> Result<Arc<runtara_component_host::isolated_tasks::TaskResult>, TaskError> {
        tokio::time::timeout(Duration::from_secs(5), self.tasks.join(id))
            .await
            .unwrap()
    }
    async fn close(&self, failed: bool) {
        assert_eq!(self.tasks.shutdown().await.is_err(), failed);
        self.admission
            .persistence
            .invocation_fences()
            .unwrap()
            .revoke_invocation_lease(&self.admission.lease)
            .await
            .unwrap();
        let root = self
            .admission
            .persistence
            .get_instance(&self.admission.lease.instance_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(root.status, runtara_core::domain::InstanceStatus::Running);
        assert!(root.output.is_none());
    }
}

#[tokio::test]
async fn initial_admission_supplies_fenced_io_and_settles_native_cancel_without_tombstone() {
    for cancel in [false, true] {
        let fx = Fixture::new(Duration::from_secs(3)).await;
        assert!(fx.admission.io().is_err());
        assert_eq!(fx.count().await, 0);
        let admission = fx.admission.clone();
        let running = Arc::new(Notify::new());
        let started = running.clone();
        let id = fx
            .tasks
            .spawn_managed(
                Box::new(move |_| {
                    Box::pin(async move {
                        let io = admission.io().unwrap();
                        io.checkpoint("child/result".into(), b"kept".to_vec())
                            .await
                            .unwrap();
                        started.notify_one();
                        if cancel {
                            std::future::pending::<()>().await;
                        }
                        InvokeExit::Completed(b"result".to_vec())
                    })
                }),
                None,
                Some(fx.admission.clone()),
            )
            .unwrap();
        tokio::time::timeout(Duration::from_secs(3), running.notified())
            .await
            .unwrap();
        if cancel {
            fx.tasks.cancel(id).unwrap();
        }
        let result = fx.join(id).await.unwrap();
        assert!(if cancel {
            matches!(result.outcome(), InvokeExit::Cancelled)
        } else {
            matches!(result.outcome(), InvokeExit::Completed(bytes) if bytes == b"result")
        });
        let stored = fx.admission.begin().await.unwrap();
        assert_eq!(stored.state, AttemptState::Settled);
        assert_eq!(&stored.fence, fx.admission.io().unwrap().fence());
        assert_eq!(fx.count().await, 1);
        assert!(
            fx.admission
                .io()
                .unwrap()
                .checkpoint("child/late".into(), vec![1])
                .await
                .is_err()
        );
        fx.close(false).await;
    }
}

// Delay/fail only the database reply, after a real transaction commits. Both
// admission bookkeeping and settlement are the production implementation.
struct ReplyBoundary {
    inner: Arc<InvocationAdmission>,
    committed: Notify,
    mode: &'static str,
}
#[async_trait::async_trait]
impl TaskLifecycle for ReplyBoundary {
    async fn admit(&self, _: TaskCancellation) -> Result<Option<InvokeExit>, TaskError> {
        self.inner
            .admit_with(async {
                let attempt = self.inner.begin().await?;
                self.committed.notify_one();
                match self.mode {
                    "dropped" | "cancelled" => {
                        std::future::pending::<FenceResult<InvocationAttempt>>().await
                    }
                    "error" => Err(InvocationFenceError::Storage("lost reply".into())),
                    "panic" => panic!("lost reply after commit"),
                    "wrong-owner" => {
                        let mut wrong = attempt;
                        wrong.fence.lease.owner = "foreign".into();
                        Ok(wrong)
                    }
                    _ => unreachable!(),
                }
            })
            .await
    }
    async fn settle(
        &self,
        outcome: Result<InvokeExit, TaskError>,
        cancel: TaskCancellation,
    ) -> Result<InvokeExit, TaskError> {
        self.inner.settle(outcome, cancel).await
    }
}

#[tokio::test]
async fn initial_admission_recovers_dropped_reply_and_fences_failure_or_panic() {
    for mode in ["dropped", "cancelled", "error", "panic", "wrong-owner"] {
        let fx = Fixture::new(Duration::from_secs(3)).await;
        let replies = Arc::new(ReplyBoundary {
            inner: fx.admission.clone(),
            committed: Notify::new(),
            mode,
        });
        let ran = Arc::new(AtomicBool::new(false));
        let child_ran = ran.clone();
        let id = fx
            .tasks
            .spawn_managed(
                Box::new(move |_| {
                    Box::pin(async move {
                        child_ran.store(true, Ordering::Release);
                        InvokeExit::Completed(vec![])
                    })
                }),
                None,
                Some(replies.clone()),
            )
            .unwrap();
        tokio::time::timeout(Duration::from_secs(3), replies.committed.notified())
            .await
            .unwrap();
        assert!(fx.admission.attempt.get().is_none());
        if mode == "cancelled" {
            let stored = fx.admission.begin().await.unwrap();
            fx.admission
                .persistence
                .invocation_fences()
                .unwrap()
                .cancel_invocation_attempt(&stored.fence)
                .await
                .unwrap();
        }
        let interrupted = matches!(mode, "dropped" | "cancelled");
        if interrupted {
            fx.tasks.cancel(id).unwrap();
        }
        let result = fx.join(id).await;
        if interrupted {
            assert!(matches!(result.unwrap().outcome(), InvokeExit::Cancelled));
            let stored = fx.admission.begin().await.unwrap();
            assert_eq!(
                stored.state,
                if mode == "cancelled" {
                    AttemptState::Cancelled
                } else {
                    AttemptState::Settled
                }
            );
            assert_eq!(stored.fence.start_id, fx.admission.start_id);
        } else {
            assert!(matches!(result, Err(TaskError::WorkerLost)));
            assert!(
                !fx.admission
                    .persistence
                    .invocation_fences()
                    .unwrap()
                    .get_invocation_lease(
                        &fx.admission.lease.tenant_id,
                        &fx.admission.lease.instance_id
                    )
                    .await
                    .unwrap()
                    .unwrap()
                    .active
            );
        }
        assert_eq!(
            fx.count().await,
            1,
            "must not allocate a second admission identity"
        );
        assert!(!ran.load(Ordering::Acquire));
        fx.close(!interrupted).await;
    }
}

#[tokio::test]
async fn initial_admission_cancel_before_poll_has_zero_attempts() {
    // On this single-thread runtime cancellation happens before the spawned
    // worker is first polled. Preparation alone cannot touch the ledger.
    let fx = Fixture::new(Duration::from_secs(3)).await;
    let id = fx
        .tasks
        .spawn_managed(
            Box::new(|_| Box::pin(async { panic!("pre-cancelled child must not run") })),
            None,
            Some(fx.admission.clone()),
        )
        .unwrap();
    fx.tasks.cancel(id).unwrap();
    assert!(matches!(
        fx.join(id).await.unwrap().outcome(),
        InvokeExit::Cancelled
    ));
    assert!(!fx.admission.started.load(Ordering::Acquire));
    assert!(fx.admission.io().is_err());
    assert_eq!(fx.count().await, 0);
    fx.close(false).await;
}

#[tokio::test]
async fn initial_admission_observes_old_lease_cancel_tombstone_without_child_io() {
    let mut fx = Fixture::new(Duration::from_secs(3)).await;
    let fences = fx.admission.persistence.invocation_fences().unwrap();
    let old = fx.admission.begin().await.unwrap();
    fences.cancel_invocation_attempt(&old.fence).await.unwrap();
    fences
        .revoke_invocation_lease(&old.fence.lease)
        .await
        .unwrap();
    let lease = fences
        .claim_invocation_lease(
            &old.fence.lease.tenant_id,
            &old.fence.lease.instance_id,
            "replay",
            Some(old.fence.lease.epoch),
        )
        .await
        .unwrap();
    fx.admission = Arc::new(InvocationAdmission::new(
        fx.admission.persistence.clone(),
        lease,
        old.fence.path.clone(),
        Duration::from_secs(3),
    ));
    let id = fx
        .tasks
        .spawn_managed(
            Box::new(|_| Box::pin(async { panic!("persisted cancelled child must not run") })),
            None,
            Some(fx.admission.clone()),
        )
        .unwrap();
    assert!(matches!(
        fx.join(id).await.unwrap().outcome(),
        InvokeExit::Cancelled
    ));
    assert_eq!(
        fx.admission.attempt.get().unwrap(),
        &InvocationAttempt {
            state: AttemptState::Cancelled,
            ..old
        }
    );
    assert!(fx.admission.io().is_err());
    assert_eq!(fx.count().await, 1);
    assert!(
        fx.admission
            .persistence
            .invocation_fences()
            .unwrap()
            .get_invocation_lease(
                &fx.admission.lease.tenant_id,
                &fx.admission.lease.instance_id
            )
            .await
            .unwrap()
            .unwrap()
            .active,
        "old cancellation must not revoke the replay owner"
    );
    fx.close(false).await;
}

#[tokio::test]
async fn initial_admission_timeout_fails_shutdown_until_root_fencing_can_be_retried() {
    let fx = Fixture::new(Duration::from_millis(40)).await;
    let pool = crate::test_support::pool().await;
    let mut hold = pool.begin().await.unwrap();
    sqlx::query("SELECT instance_id FROM instances WHERE instance_id=$1 FOR UPDATE")
        .bind(&fx.admission.lease.instance_id)
        .execute(&mut *hold)
        .await
        .unwrap();
    let id = fx
        .tasks
        .spawn_managed(
            Box::new(|_| Box::pin(async { panic!("timed-out admission must not run") })),
            None,
            Some(fx.admission.clone()),
        )
        .unwrap();
    assert!(matches!(fx.join(id).await, Err(TaskError::WorkerLost)));
    assert!(fx.admission.failed.load(Ordering::Acquire));
    assert!(fx.admission.io().is_err());
    assert!(matches!(
        fx.tasks.shutdown().await,
        Err(TaskError::WorkerLost)
    ));
    hold.rollback().await.unwrap();
    // The first revoke also timed out on the held row. The owner must retry,
    // rather than treating a bounded failure as proof of durable fencing.
    fx.close(true).await;
    assert!(
        !fx.admission
            .persistence
            .invocation_fences()
            .unwrap()
            .get_invocation_lease(
                &fx.admission.lease.tenant_id,
                &fx.admission.lease.instance_id
            )
            .await
            .unwrap()
            .unwrap()
            .active
    );
}

#[tokio::test]
async fn uncertain_admission_resolution_failure_retains_failed_shutdown() {
    let fx = Fixture::new(Duration::from_secs(1)).await;
    let pool = crate::test_support::pool().await;
    let replies = Arc::new(ReplyBoundary {
        inner: fx.admission.clone(),
        committed: Notify::new(),
        mode: "dropped",
    });
    let id = fx
        .tasks
        .spawn_managed(
            Box::new(|_| Box::pin(async { panic!("unresolved admission must not start a child") })),
            None,
            Some(replies.clone()),
        )
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), replies.committed.notified())
        .await
        .unwrap();
    let mut hold = pool.begin().await.unwrap();
    sqlx::query("SELECT instance_id FROM instances WHERE instance_id=$1 FOR UPDATE")
        .bind(&fx.admission.lease.instance_id)
        .execute(&mut *hold)
        .await
        .unwrap();
    assert!(!fx.admission.failed.load(Ordering::Acquire));
    assert!(fx.admission.attempt.get().is_none());
    fx.tasks.cancel(id).unwrap();
    assert!(matches!(fx.join(id).await, Err(TaskError::WorkerLost)));
    assert!(fx.admission.io().is_err());
    hold.rollback().await.unwrap();
    assert_eq!(fx.count().await, 1);
    fx.close(true).await;
}
