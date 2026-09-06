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
