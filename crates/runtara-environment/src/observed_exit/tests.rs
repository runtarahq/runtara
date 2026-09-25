use super::*;
use crate::container_registry::{ContainerInfo, ContainerRegistry};
use runtara_core::persistence::inputs::{
    InputAuthority, InputClosure, InputRequestSpec, InputState,
};
use std::sync::Arc;

struct Fixture {
    pool: PgPool,
    persistence: Arc<dyn Persistence>,
    container: ContainerInfo,
    request: String,
    fault: String,
}

impl Fixture {
    async fn new() -> Self {
        let (persistence, instance) = crate::test_support::running_instance("exit-input").await;
        let pool = crate::test_support::pool().await;
        let tenant = persistence
            .get_instance_meta(&instance)
            .await
            .unwrap()
            .unwrap()
            .tenant_id;
        let request = persistence
            .input_requests()
            .unwrap()
            .register_input(
                &InputAuthority::Root {
                    tenant_id: tenant.clone(),
                    instance_id: instance.clone(),
                },
                &InputRequestSpec {
                    signal_id: "wait".into(),
                    deadline: None,
                    response_schema: None,
                    metadata: serde_json::json!({}),
                },
            )
            .await
            .unwrap()
            .request_id;
        let container = ContainerInfo {
            container_id: uuid::Uuid::new_v4().to_string(),
            launch_id: uuid::Uuid::new_v4().to_string(),
            instance_id: instance.clone(),
            tenant_id: tenant,
            binary_path: "/fixture/exited.wasm".into(),
            started_at: chrono::Utc::now(),
            timeout_seconds: None,
        };
        ContainerRegistry::new(pool.clone())
            .register(&container)
            .await
            .unwrap();
        let fault = format!("exit_fault_{}", uuid::Uuid::new_v4().simple());
        // nextval is not rolled back, proving the automatic monitor tried the
        // failing write before the test restores storage.
        sqlx::query(&format!("CREATE SEQUENCE {fault}"))
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(&format!("CREATE FUNCTION {fault}() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM nextval('{fault}'); RAISE EXCEPTION 'injected terminal input closure failure'; END $$"))
            .execute(&pool).await.unwrap();
        sqlx::query(&format!("CREATE TRIGGER {fault} BEFORE UPDATE ON instance_input_requests FOR EACH ROW WHEN (NEW.instance_id = '{instance}' AND NEW.closure_reason = 'instance_terminated') EXECUTE FUNCTION {fault}()"))
            .execute(&pool).await.unwrap();
        Self {
            pool,
            persistence,
            container,
            request,
            fault,
        }
    }

    fn intent(&self) -> ObservedExit {
        ObservedExit::unreported(
            false,
            "managed abandonment could not be confirmed".into(),
            None,
        )
    }

    async fn wait_for_failed_write(&self) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let attempted: bool =
                    sqlx::query_scalar(&format!("SELECT is_called FROM {}", self.fault))
                        .fetch_one(&self.pool)
                        .await
                        .unwrap();
                if attempted {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("monitor must attempt terminal closure");
    }

    async fn restore(&self) {
        for sql in [
            format!("DROP TRIGGER {} ON instance_input_requests", self.fault),
            format!("DROP FUNCTION {}()", self.fault),
            format!("DROP SEQUENCE {}", self.fault),
        ] {
            sqlx::query(&sql).execute(&self.pool).await.unwrap();
        }
    }

    async fn retained_state(&self) -> InputState {
        self.persistence
            .input_requests()
            .unwrap()
            .get_input(
                &self.container.tenant_id,
                &self.container.instance_id,
                &self.request,
            )
            .await
            .unwrap()
            .state
    }

    async fn assert_closed(&self) {
        let root = self
            .persistence
            .get_instance_meta(&self.container.instance_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(root.status, InstanceStatus::Failed);
        assert!(root.sleep_until.is_none());
        assert!(matches!(
            self.retained_state().await,
            InputState::Closed {
                reason: InputClosure::InstanceTerminated,
                ..
            }
        ));
        assert_eq!(
            self.persistence
                .input_requests()
                .unwrap()
                .list_inputs(
                    &self.container.tenant_id,
                    std::slice::from_ref(&self.container.instance_id),
                    0,
                    10,
                )
                .await
                .unwrap()
                .total_count,
            0
        );
        assert!(
            ContainerRegistry::new(self.pool.clone())
                .get(&self.container.instance_id)
                .await
                .unwrap()
                .is_none()
        );
    }

    fn spawn_retry(&self) -> tokio::task::JoinHandle<bool> {
        let pool = self.pool.clone();
        let persistence = self.persistence.clone();
        let handle = self.container.runner_handle();
        let intent = self.intent();
        tokio::spawn(async move {
            settle_with_retry(&pool, persistence.as_ref(), &handle, intent).await
        })
    }
}

#[tokio::test]
async fn terminal_storage_failure_keeps_the_request_and_retries_until_closed() {
    let fx = Fixture::new().await;
    let monitor = fx.spawn_retry();
    fx.wait_for_failed_write().await;
    assert_eq!(fx.retained_state().await, InputState::Open);
    let pending: bool = sqlx::query_scalar(
        "SELECT observed_exit IS NOT NULL FROM container_registry WHERE instance_id = $1",
    )
    .bind(&fx.container.instance_id)
    .fetch_one(&fx.pool)
    .await
    .unwrap();
    assert!(pending, "failed cleanup cannot discard its recovery intent");
    fx.restore().await;
    assert!(
        tokio::time::timeout(Duration::from_secs(5), monitor)
            .await
            .unwrap()
            .unwrap()
    );
    fx.assert_closed().await;
}

#[tokio::test]
async fn restart_recovery_closes_a_known_failure_instead_of_relaunching_its_wait() {
    let fx = Fixture::new().await;
    let handle = fx.container.runner_handle();
    assert!(record(&fx.pool, &handle, &fx.intent()).await.unwrap());
    assert!(
        settle(&fx.pool, fx.persistence.as_ref(), &handle)
            .await
            .is_err()
    );
    fx.restore().await;
    let outcome = crate::recovery::recover_registered_with(
        &fx.pool,
        fx.persistence.as_ref(),
        &fx.container,
        crate::recovery::RecoveryPolicy::default(),
    )
    .await
    .unwrap();
    assert_eq!(outcome, Some(crate::recovery::RecoveryOutcome::Failed));
    fx.assert_closed().await;
}

#[tokio::test]
async fn an_old_monitor_cannot_close_or_delete_a_replacement_during_retry() {
    let fx = Fixture::new().await;
    let monitor = fx.spawn_retry();
    fx.wait_for_failed_write().await;
    let mut replacement = fx.container.clone();
    replacement.container_id = uuid::Uuid::new_v4().to_string();
    let registry = ContainerRegistry::new(fx.pool.clone());
    registry.register(&replacement).await.unwrap();
    fx.restore().await;
    assert!(
        !tokio::time::timeout(Duration::from_secs(5), monitor)
            .await
            .unwrap()
            .unwrap()
    );
    assert_eq!(
        registry
            .get(&fx.container.instance_id)
            .await
            .unwrap()
            .unwrap()
            .container_id,
        replacement.container_id
    );
    let pending: bool = sqlx::query_scalar(
        "SELECT observed_exit IS NOT NULL FROM container_registry WHERE instance_id = $1",
    )
    .bind(&fx.container.instance_id)
    .fetch_one(&fx.pool)
    .await
    .unwrap();
    assert!(
        !pending,
        "a new physical attempt cannot inherit the old exit"
    );
    assert_eq!(fx.retained_state().await, InputState::Open);
    assert_eq!(
        fx.persistence
            .get_instance_meta(&fx.container.instance_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        InstanceStatus::Running
    );
}
