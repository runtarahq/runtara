use super::*;
use runtara_component_host::InvokeExit;
use runtara_component_host::isolated_tasks::TaskError;

async fn io(fx: &Fixture) -> Arc<InvocationIo> {
    let root = fx.persistence.get_instance(&fx.id).await.unwrap().unwrap();
    let fences = fx.persistence.invocation_fences().unwrap();
    let lease = fences
        .claim_invocation_lease(&root.tenant_id, &fx.id, "io-test", None)
        .await
        .unwrap();
    let attempt = fences
        .begin_invocation_attempt(&lease, "parent/child", "one")
        .await
        .unwrap();
    Arc::new(
        InvocationIo::new(
            fx.persistence.clone(),
            attempt.fence,
            Duration::from_secs(5),
        )
        .unwrap(),
    )
}
async fn child(fx: &Fixture, io: Arc<InvocationIo>) -> Arc<ScopedRuntimeHost> {
    let (tokens, _) = fx.child().await;
    fx.owner
        .child_fenced(
            b"input".to_vec(),
            Arc::new(Keys("child/")),
            tokens.cancel.clone(),
            io,
        )
        .unwrap()
}
async fn revoke(fx: &Fixture, io: &InvocationIo) {
    fx.close().await;
    fx.persistence
        .invocation_fences()
        .unwrap()
        .revoke_invocation_lease(&io.fence().lease)
        .await
        .unwrap();
}

#[tokio::test]
async fn fenced_child_preserves_checkpoint_signal_retry_and_event_semantics() {
    let fx = Fixture::new().await;
    let io = io(&fx).await;
    let child = child(&fx, io.clone()).await;
    fx.persistence
        .insert_signal(&fx.id, CoreSignal::Pause, b"")
        .await
        .unwrap();
    fx.persistence
        .put_custom_signal(&fx.id, "child/key", b"custom")
        .await
        .unwrap();
    let probe = child.checkpoint("child/key".into(), vec![]).await.unwrap();
    assert!(!probe.found && probe.custom_signal.is_none());
    assert!(probe.pending_signal.is_some());
    assert_eq!(
        child.poll_custom_signal("child/key".into()).await.unwrap(),
        Some(b"custom".to_vec())
    );
    let first = child
        .checkpoint("child/key".into(), b"first".to_vec())
        .await
        .unwrap();
    assert!(!first.found);
    assert_eq!(first.custom_signal.unwrap().payload, b"custom");
    let hit = child
        .checkpoint("child/key".into(), b"second".to_vec())
        .await
        .unwrap();
    assert!(hit.found);
    assert_eq!(hit.state, b"first");
    assert_eq!(
        child.get_checkpoint("child/key".into()).await.unwrap(),
        Some(b"first".to_vec())
    );
    child
        .record_retry_attempt("child/key".into(), 1, Some("test".into()))
        .await
        .unwrap();
    assert!(
        fx.persistence
            .load_checkpoint(&fx.id, "child/key::retry::1")
            .await
            .unwrap()
            .is_some()
    );
    child
        .custom_event("debug".into(), b"payload".to_vec())
        .await
        .unwrap();
    child.heartbeat().await.unwrap();
    let events = fx
        .persistence
        .list_events(&fx.id, &ListEventsFilter::default(), 10, 0)
        .await
        .unwrap();
    assert_eq!(events.len(), 2);
    assert!(
        events
            .iter()
            .all(|e| e.checkpoint_id.as_deref() == Some("parent/child"))
    );
    child.complete(b"child result".to_vec()).await.unwrap();
    assert!(!io.failed());
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
    assert!(
        fx.persistence
            .get_pending_signal(&fx.id)
            .await
            .unwrap()
            .is_some()
    );
    revoke(&fx, &io).await;
}

#[tokio::test]
async fn fenced_child_rejections_prevent_all_writes_and_latch_host_failure() {
    for cancel in [false, true] {
        let fx = Fixture::new().await;
        let io = io(&fx).await;
        let child = child(&fx, io.clone()).await;
        let fences = fx.persistence.invocation_fences().unwrap();
        if cancel {
            fences.cancel_invocation_attempt(io.fence()).await.unwrap();
        } else {
            fences
                .revoke_invocation_lease(&io.fence().lease)
                .await
                .unwrap();
        }
        assert!(
            child
                .checkpoint("child/late".into(), b"late".to_vec())
                .await
                .is_err()
        );
        assert_eq!(io.failed(), !cancel);
        assert!(child.get_checkpoint("child/late".into()).await.is_err());
        assert!(child.poll_custom_signal("child/late".into()).await.is_err());
        assert!(
            child
                .record_retry_attempt("child/late".into(), 1, None)
                .await
                .is_err()
        );
        assert!(
            child
                .durable_sleep_checkpoint("child/late".into(), vec![], 0)
                .await
                .is_err()
        );
        assert!(child.custom_event("late".into(), vec![]).await.is_err());
        assert!(child.heartbeat().await.is_err());
        // Ignoring an error cannot reset the host latch through a success callback.
        assert!(child.complete(b"ignore error".to_vec()).await.is_err());
        assert_eq!(
            fx.persistence
                .count_checkpoints(&fx.id, None, None, None)
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            fx.persistence
                .count_events(&fx.id, &ListEventsFilter::default())
                .await
                .unwrap(),
            0
        );
        revoke(&fx, &io).await;
    }
}

#[tokio::test]
async fn fenced_sleep_heartbeats_stop_after_persisted_cancellation_or_revocation() {
    for cancel in [false, true] {
        let fx = Fixture::new().await;
        let io = io(&fx).await;
        let child = child(&fx, io.clone()).await;
        let running = tokio::spawn(async move {
            child
                .durable_sleep_checkpoint("child/sleep".into(), b"sleep state".to_vec(), 60_000)
                .await
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if fx
                    .persistence
                    .load_checkpoint(&fx.id, "child/sleep")
                    .await
                    .unwrap()
                    .is_some()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        let fences = fx.persistence.invocation_fences().unwrap();
        if cancel {
            fences.cancel_invocation_attempt(io.fence()).await.unwrap();
        } else {
            fences
                .revoke_invocation_lease(&io.fence().lease)
                .await
                .unwrap();
        }
        assert!(
            tokio::time::timeout(Duration::from_secs(5), running)
                .await
                .unwrap()
                .unwrap()
                .is_err()
        );
        assert_eq!(io.failed(), !cancel);
        assert_eq!(
            fx.persistence
                .count_events(&fx.id, &ListEventsFilter::default())
                .await
                .unwrap(),
            0
        );
        let root = fx.persistence.get_instance(&fx.id).await.unwrap().unwrap();
        assert_eq!(root.status, InstanceStatus::Running);
        assert!(root.sleep_until.is_none());
        assert_eq!(root.checkpoint_id.as_deref(), Some("child/sleep"));
        revoke(&fx, &io).await;
    }
}

#[tokio::test]
async fn fenced_sleep_observes_root_commands_without_consuming_them() {
    for command in [CoreSignal::Cancel, CoreSignal::Shutdown] {
        let fx = Fixture::with_poll_interval(Duration::from_secs(3600)).await;
        let io = io(&fx).await;
        let child = child(&fx, io.clone()).await;
        assert!(!child.check_signals().await.unwrap());
        fx.persistence
            .insert_signal(&fx.id, command, b"")
            .await
            .unwrap();
        tokio::time::timeout(
            Duration::from_secs(5),
            child.durable_sleep_checkpoint("child/sleep".into(), vec![], 60_000),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(child.check_signals().await.unwrap());
        assert!(
            fx.persistence
                .get_pending_signal(&fx.id)
                .await
                .unwrap()
                .is_some()
        );
        assert!(!io.failed());
        assert_eq!(fx.status().await, InstanceStatus::Running);
        assert_eq!(
            fx.persistence
                .count_events(&fx.id, &ListEventsFilter::default())
                .await
                .unwrap(),
            1
        );
        assert!(!fx.owner.root.sleep_interrupted.load(Ordering::SeqCst));
        revoke(&fx, &io).await;
    }
}

#[tokio::test]
async fn fenced_storage_failure_cannot_be_caught_into_success_or_report_an_event() {
    use runtara_core::instance_handlers::InstanceEventObserver;
    use std::sync::atomic::AtomicUsize;
    struct Observer(AtomicUsize);
    impl InstanceEventObserver for Observer {
        fn on_event_persisted(&self, _: Option<&str>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    let fx = Fixture::new().await;
    let io = io(&fx).await;
    let observer = Arc::new(Observer(AtomicUsize::new(0)));
    let mut state = InstanceHandlerState::new(fx.persistence.clone());
    state.event_observer = Some(observer.clone());
    let owner = Arc::new(ScopedRuntimeOwner::new(Arc::new(
        PersistenceRuntimeHost::new(Arc::new(state), fx.id.clone(), false),
    )));
    let run_owner = owner.clone();
    let run_io = io.clone();
    let id = fx
        .tasks
        .spawn_managed(
            move |cancel| async move {
                let child = run_owner
                    .child_fenced(vec![], Arc::new(Keys("child/")), cancel, run_io)
                    .unwrap();
                child
                    .custom_event("valid".into(), b"one".to_vec())
                    .await
                    .unwrap();
                // PostgreSQL text rejects NUL. Pretend the guest catches and ignores
                // this error, including the rejected terminal callback, then succeeds.
                assert!(
                    child
                        .custom_event("invalid\0subtype".into(), vec![])
                        .await
                        .is_err()
                );
                let _ = child.complete(b"ignored error".to_vec()).await;
                InvokeExit::Completed(b"exported success".to_vec())
            },
            None,
            Some(io.clone()),
        )
        .unwrap();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(5), fx.tasks.join(id))
            .await
            .unwrap(),
        Err(TaskError::WorkerLost)
    ));
    assert!(io.failed());
    assert_eq!(observer.0.load(Ordering::SeqCst), 1);
    assert_eq!(
        fx.persistence
            .count_events(&fx.id, &ListEventsFilter::default())
            .await
            .unwrap(),
        1
    );
    assert!(
        !fx.persistence
            .invocation_fences()
            .unwrap()
            .get_invocation_lease(&io.fence().lease.tenant_id, &fx.id)
            .await
            .unwrap()
            .unwrap()
            .active
    );
    assert!(matches!(
        fx.tasks.shutdown().await,
        Err(TaskError::WorkerLost)
    ));
    fx.owner.close_after_cleanup().unwrap();
    owner.close_after_cleanup().unwrap();
}

#[tokio::test]
async fn fenced_io_rejects_another_root_and_default_children_do_not_create_leases() {
    let fx = Fixture::new().await;
    let (live, _) = fx.child().await;
    live.checkpoint("child/live".into(), b"live".to_vec())
        .await
        .unwrap();
    let root = fx.persistence.get_instance(&fx.id).await.unwrap().unwrap();
    assert!(
        fx.persistence
            .invocation_fences()
            .unwrap()
            .get_invocation_lease(&root.tenant_id, &fx.id)
            .await
            .unwrap()
            .is_none()
    );
    let io = io(&fx).await;
    let other = Fixture::new().await;
    let (tokens, _) = other.child().await;
    assert!(
        other
            .owner
            .child_fenced(
                vec![],
                Arc::new(Keys("child/")),
                tokens.cancel.clone(),
                io.clone()
            )
            .is_err()
    );
    other.close().await;
    revoke(&fx, &io).await;
}

#[tokio::test]
async fn managed_fenced_io_cancellation_overrides_a_caught_error_and_success() {
    let fx = Fixture::new().await;
    let io = io(&fx).await;
    let (ready, entered) = tokio::sync::oneshot::channel();
    let (release, resume) = tokio::sync::oneshot::channel();
    let owner = fx.owner.clone();
    let run_io = io.clone();
    let id = fx
        .tasks
        .spawn_managed(
            move |cancel| async move {
                let child = owner
                    .child_fenced(vec![], Arc::new(Keys("child/")), cancel, run_io)
                    .unwrap();
                child
                    .durable_sleep_checkpoint("child/zero".into(), vec![], 0)
                    .await
                    .unwrap();
                ready.send(()).unwrap();
                resume.await.unwrap();
                assert!(
                    child
                        .checkpoint("child/late".into(), b"late".to_vec())
                        .await
                        .is_err()
                );
                InvokeExit::Completed(b"ignored cancellation".to_vec())
            },
            None,
            Some(io.clone()),
        )
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), entered)
        .await
        .unwrap()
        .unwrap();
    fx.persistence
        .invocation_fences()
        .unwrap()
        .cancel_invocation_attempt(io.fence())
        .await
        .unwrap();
    release.send(()).unwrap();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(5), fx.tasks.join(id))
            .await
            .unwrap()
            .unwrap()
            .outcome(),
        InvokeExit::Cancelled
    ));
    assert!(!io.failed());
    assert!(
        fx.persistence
            .load_checkpoint(&fx.id, "child/late")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        fx.persistence
            .load_checkpoint(&fx.id, "child/zero")
            .await
            .unwrap()
            .unwrap()
            .state
            .is_empty()
    );
    assert_eq!(
        fx.persistence
            .count_events(&fx.id, &ListEventsFilter::default())
            .await
            .unwrap(),
        0
    );
    revoke(&fx, &io).await;
}

#[tokio::test]
async fn managed_fenced_io_bounds_blocked_control_and_retains_failure_for_root_cleanup() {
    let fx = Fixture::new().await;
    let admitted = io(&fx).await;
    let io = Arc::new(
        InvocationIo::new(
            fx.persistence.clone(),
            admitted.fence().clone(),
            Duration::from_millis(50),
        )
        .unwrap(),
    );
    let pool = crate::test_support::pool().await;
    let mut hold = pool.begin().await.unwrap();
    sqlx::query("SELECT instance_id FROM instances WHERE instance_id=$1 FOR UPDATE")
        .bind(&fx.id)
        .fetch_one(&mut *hold)
        .await
        .unwrap();
    let started = Arc::new(AtomicBool::new(false));
    let run_started = started.clone();
    let id = fx
        .tasks
        .spawn_managed(
            move |_| async move {
                run_started.store(true, Ordering::SeqCst);
                InvokeExit::Completed(vec![])
            },
            None,
            Some(io.clone()),
        )
        .unwrap();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), fx.tasks.join(id))
            .await
            .unwrap(),
        Err(TaskError::WorkerLost)
    ));
    assert!(io.failed());
    assert!(matches!(
        fx.tasks.shutdown().await,
        Err(TaskError::WorkerLost)
    ));
    assert!(!started.load(Ordering::SeqCst));
    // The root supervisor must retry revocation when storage becomes reachable;
    // neither a timed-out revocation nor a failed shutdown grants release/relaunch.
    hold.rollback().await.unwrap();
    fx.persistence
        .invocation_fences()
        .unwrap()
        .revoke_invocation_lease(&io.fence().lease)
        .await
        .unwrap();
    assert!(
        !fx.persistence
            .invocation_fences()
            .unwrap()
            .get_invocation_lease(&io.fence().lease.tenant_id, &fx.id)
            .await
            .unwrap()
            .unwrap()
            .active
    );
    fx.owner.close_after_cleanup().unwrap();
}
