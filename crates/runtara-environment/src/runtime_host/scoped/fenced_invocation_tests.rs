//! Actual prepared WASM child with the runtime's fenced IO and lifecycle.
use super::*;
use runtara_component_host::isolated_tasks::TaskError;
use runtara_core::persistence::invocations::*;

struct FencedScopes {
    scopes: Arc<ScopedInvocationFactory>,
    io: Arc<InvocationIo>,
}
impl InvocationScopeFactory for FencedScopes {
    fn prepare_child(
        &self,
        request: &StartRequest,
    ) -> Result<runtara_component_host::ChildInvocationScope, ExecutionError> {
        self.scopes.prepare_fenced_child(request, self.io.clone())
    }
}

struct DurabilityAuthority(Option<bool>);
impl InvocationAuthority for DurabilityAuthority {
    fn authorize(&self, request: &StartRequest) -> Result<AuthorizedChild, ExecutionError> {
        let mut authorized = Authority.authorize(request)?;
        authorized.durable = self.0;
        Ok(authorized)
    }
}

#[tokio::test]
async fn prepared_wasm_factory_admits_only_durable_calls_without_memoizing_results() {
    for durable in [None, Some(false), Some(true)] {
        let fx = Fixture::new().await;
        let root = fx.persistence.get_instance(&fx.id).await.unwrap().unwrap();
        let fences = fx.persistence.invocation_fences().unwrap();
        let lease = fences
            .claim_invocation_lease(&root.tenant_id, &fx.id, "auto-admission", None)
            .await
            .unwrap();
        let scopes = Arc::new(
            ScopedInvocationFactory::new(
                fx.owner.clone(),
                Arc::new(DurabilityAuthority(durable)),
                settings(
                    Instant::now() + Duration::from_secs(10),
                    Arc::new(AtomicBool::new(false)),
                ),
            )
            .with_invocation_lease(lease.clone(), Duration::from_secs(3))
            .unwrap(),
        );
        let launcher = launcher(&fx, scopes).await;
        let pool = crate::test_support::pool().await;
        for i in 1..=2 {
            let prepared = launcher.prepare(request("copy")).unwrap();
            assert_eq!(prepared.lifecycle.is_some(), durable == Some(true));
            // Synchronous preparation performs no admission transaction.
            let before: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM invocation_attempts WHERE instance_id=$1")
                    .bind(&fx.id)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(before, if durable == Some(true) { i - 1 } else { 0 });
            let id = fx
                .tasks
                .spawn_managed(prepared.run, prepared.cleanup, prepared.lifecycle)
                .unwrap();
            assert!(
                matches!(tokio::time::timeout(Duration::from_secs(5), fx.tasks.join(id))
                .await.unwrap().unwrap().outcome(), InvokeExit::Completed(bytes) if *bytes == request("copy").input)
            );
            assert_eq!(
                fx.persistence
                    .count_events(&fx.id, &ListEventsFilter::default())
                    .await
                    .unwrap(),
                i
            );
            let states: Vec<(String, String)> = sqlx::query_as(
                "SELECT start_id,state FROM invocation_attempts WHERE instance_id=$1",
            )
            .bind(&fx.id)
            .fetch_all(&pool)
            .await
            .unwrap();
            assert_eq!(
                states.len(),
                if durable == Some(true) { i as usize } else { 0 }
            );
            assert!(states.iter().all(|(_, state)| state == "settled"));
            if states.len() == 2 {
                assert_ne!(states[0].0, states[1].0);
            }
        }
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
        fx.close().await;
        fences.revoke_invocation_lease(&lease).await.unwrap();
    }
}

#[tokio::test]
async fn prepared_wasm_initial_admission_skips_initializer_for_cancelled_replay() {
    let fx = Fixture::new().await;
    let root = fx.persistence.get_instance(&fx.id).await.unwrap().unwrap();
    let fences = fx.persistence.invocation_fences().unwrap();
    let old = fences
        .claim_invocation_lease(&root.tenant_id, &fx.id, "old", None)
        .await
        .unwrap();
    let attempt = fences
        .begin_invocation_attempt(&old, "parent/child", "cancelled")
        .await
        .unwrap();
    fences
        .cancel_invocation_attempt(&attempt.fence)
        .await
        .unwrap();
    fences.revoke_invocation_lease(&old).await.unwrap();
    let lease = fences
        .claim_invocation_lease(&root.tenant_id, &fx.id, "new", Some(old.epoch))
        .await
        .unwrap();
    let scopes = Arc::new(
        ScopedInvocationFactory::new(
            fx.owner.clone(),
            Arc::new(Authority),
            settings(
                Instant::now() + Duration::from_secs(5),
                Arc::new(AtomicBool::new(false)),
            ),
        )
        .with_invocation_lease(lease.clone(), Duration::from_secs(3))
        .unwrap(),
    );
    let prepared = launcher(&fx, scopes)
        .await
        .prepare(request("copy"))
        .unwrap();
    let id = fx
        .tasks
        .spawn_managed(prepared.run, prepared.cleanup, prepared.lifecycle)
        .unwrap();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(5), fx.tasks.join(id))
            .await
            .unwrap()
            .unwrap()
            .outcome(),
        InvokeExit::Cancelled
    ));
    assert_eq!(
        fx.persistence
            .count_events(&fx.id, &ListEventsFilter::default())
            .await
            .unwrap(),
        0
    );
    assert_eq!(fx.status().await, InstanceStatus::Running);
    fx.close().await;
    fences.revoke_invocation_lease(&lease).await.unwrap();
}

#[tokio::test]
async fn prepared_wasm_child_uses_fenced_initializer_io_and_supervised_settlement() {
    for mode in ["complete", "cancel", "revoked"] {
        let fx = Fixture::new().await;
        let root = fx.persistence.get_instance(&fx.id).await.unwrap().unwrap();
        let fences = fx.persistence.invocation_fences().unwrap();
        let lease = fences
            .claim_invocation_lease(&root.tenant_id, &fx.id, "fenced-wasm", None)
            .await
            .unwrap();
        let attempt = fences
            .begin_invocation_attempt(&lease, "parent/child", "one")
            .await
            .unwrap();
        let io = Arc::new(
            InvocationIo::new(
                fx.persistence.clone(),
                attempt.fence,
                Duration::from_secs(5),
            )
            .unwrap(),
        );
        let scopes = factory(
            &fx,
            settings(
                Instant::now() + Duration::from_secs(5),
                Arc::new(AtomicBool::new(false)),
            ),
        );
        if mode == "complete" {
            for field in ["path", "root"] {
                let mut wrong = io.fence().clone();
                if field == "path" {
                    wrong.path = "other/child".into();
                } else {
                    wrong.lease.instance_id = "other-root".into();
                }
                let wrong = Arc::new(
                    InvocationIo::new(fx.persistence.clone(), wrong, Duration::from_secs(5))
                        .unwrap(),
                );
                assert!(matches!(
                    scopes.prepare_fenced_child(&request("copy"), wrong),
                    Err(ExecutionError::InvalidContext)
                ));
            }
            let mut wrong = request("copy");
            wrong.binding = "unapproved".into();
            assert!(matches!(
                scopes.prepare_fenced_child(&wrong, io.clone()),
                Err(ExecutionError::InvalidContext)
            ));
        }
        let launcher = PreparedInvocationLauncher::new(
            fx.executor.clone(),
            catalog(&fx, None).await,
            Arc::new(FencedScopes {
                scopes,
                io: io.clone(),
            }),
        )
        .unwrap();
        let prepared = launcher.prepare(request("copy")).unwrap();
        if mode == "cancel" {
            fences.cancel_invocation_attempt(io.fence()).await.unwrap();
        }
        if mode == "revoked" {
            fences.revoke_invocation_lease(&lease).await.unwrap();
        }
        let id = fx
            .tasks
            .spawn_managed(prepared.run, prepared.cleanup, prepared.lifecycle)
            .unwrap();
        let result = tokio::time::timeout(Duration::from_secs(5), fx.tasks.join(id))
            .await
            .unwrap();
        match mode {
            "complete" => {
                assert!(
                    matches!(result.unwrap().outcome(), InvokeExit::Completed(bytes) if *bytes == request("copy").input)
                );
                assert_eq!(
                    fences
                        .begin_invocation_attempt(&lease, "parent/child", "one")
                        .await
                        .unwrap()
                        .state,
                    AttemptState::Settled
                );
            }
            "cancel" => assert!(matches!(result.unwrap().outcome(), InvokeExit::Cancelled)),
            "revoked" => assert!(matches!(result, Err(TaskError::WorkerLost))),
            _ => unreachable!(),
        }
        assert_eq!(
            fx.persistence
                .count_events(&fx.id, &ListEventsFilter::default())
                .await
                .unwrap(),
            i64::from(mode == "complete")
        );
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
        if mode == "revoked" {
            assert!(io.failed());
            assert!(matches!(
                fx.tasks.shutdown().await,
                Err(TaskError::WorkerLost)
            ));
            fx.owner.close_after_cleanup().unwrap();
        } else {
            assert!(!io.failed());
            fx.close().await;
            fences.revoke_invocation_lease(&lease).await.unwrap();
        }
    }
}
