//! Shared runtime clients must keep each operation in its caller's tenant.
use runtara_core::{TenantId, persistence::Persistence};
use runtara_environment::{handlers::EnvironmentHandlerState, runner::MockRunner};
use runtara_server::{
    environment_client::EnvironmentClient,
    runtime_client::{RuntimeClient, RuntimeClientConfig, RuntimeError},
    runtime_types::{GetTenantMetricsOptions, ListInstancesOptions, RegisterImageStreamOptions},
};
use runtara_store_postgres::PostgresPersistence;
use std::sync::Arc;

#[tokio::test]
async fn one_client_serves_both_tenants_without_crossing_resources() {
    let url =
        std::env::var("TEST_ENVIRONMENT_DATABASE_URL").expect("isolated runtime database required");
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    runtara_environment::migrations::run(&pool).await.unwrap();
    let store = Arc::new(PostgresPersistence::new(pool.clone()));
    let dir = tempfile::tempdir().unwrap();
    let state = Arc::new(EnvironmentHandlerState::new(
        pool.clone(),
        store.clone(),
        Arc::new(MockRunner::new()),
        dir.path().into(),
    ));
    let client = Arc::new(RuntimeClient::new(
        state.clone(),
        RuntimeClientConfig::new(Default::default()),
    ));
    let environment = EnvironmentClient::new(state);
    let tenants = [
        TenantId::new(format!("a-{}", uuid::Uuid::new_v4())).unwrap(),
        TenantId::new(format!("b-{}", uuid::Uuid::new_v4())).unwrap(),
    ];
    let ids = [
        uuid::Uuid::new_v4().to_string(),
        uuid::Uuid::new_v4().to_string(),
    ];
    let mut images = Vec::new();
    for (tenant, id) in tenants.iter().zip(&ids) {
        store.register_instance(tenant, id).await.unwrap();
        store
            .save_checkpoint(tenant, id, "checkpoint", b"private")
            .await
            .unwrap();
        let image = client
            .register_image_stream(
                tenant,
                RegisterImageStreamOptions::new(tenant.as_str(), "shared-name", 8),
                &b"artifact"[..],
            )
            .await
            .unwrap();
        images.push(image.image_id);
    }
    // Concurrent calls share the same client and connection pool.
    let (a, b) = tokio::join!(
        client.get_instance_info(&tenants[0], &ids[0]),
        client.get_instance_info(&tenants[1], &ids[1])
    );
    assert_eq!(a.unwrap().tenant_id, tenants[0].as_str());
    assert_eq!(b.unwrap().tenant_id, tenants[1].as_str());
    for own in 0..2 {
        let tenant = &tenants[own];
        let foreign = &ids[1 - own];
        let missing = uuid::Uuid::new_v4().to_string();
        assert_eq!(
            client
                .list_instances(tenant, None, 100)
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            client
                .list_checkpoints(tenant, &ids[own], None, None)
                .await
                .unwrap()
                .total_count,
            1
        );
        assert!(client.list_events(tenant, &ids[own], None).await.is_ok());
        assert!(
            client
                .list_step_summaries(tenant, &ids[own], None)
                .await
                .is_ok()
        );
        assert!(
            client
                .get_scope_ancestors(tenant, &ids[own], "scope")
                .await
                .is_ok()
        );
        assert_eq!(
            client
                .find_image_by_name(tenant, "shared-name")
                .await
                .unwrap(),
            Some(images[own].clone())
        );
        assert_eq!(
            client.list_images(tenant, 100).await.unwrap().images.len(),
            1
        );
        assert!(
            client
                .image_artifact_present(tenant, &images[own])
                .await
                .unwrap()
        );
        assert!(
            client
                .get_image(tenant, &images[1 - own])
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            !client
                .image_artifact_present(tenant, &images[1 - own])
                .await
                .unwrap()
        );
        assert_eq!(
            client
                .get_tenant_metrics(tenant, GetTenantMetricsOptions::new(tenant.as_str()))
                .await
                .unwrap()
                .tenant_id,
            tenant.as_str()
        );
        for id in [foreign, &missing] {
            assert!(matches!(
                client.get_instance_info(tenant, id).await,
                Err(RuntimeError::InstanceNotFound(_))
            ));
            assert!(matches!(
                client.list_checkpoints(tenant, id, None, None).await,
                Err(RuntimeError::InstanceNotFound(_))
            ));
            assert!(matches!(
                client.list_events(tenant, id, None).await,
                Err(RuntimeError::InstanceNotFound(_))
            ));
            assert!(matches!(
                client.list_step_summaries(tenant, id, None).await,
                Err(RuntimeError::InstanceNotFound(_))
            ));
            assert!(matches!(
                client.get_scope_ancestors(tenant, id, "scope").await,
                Err(RuntimeError::InstanceNotFound(_))
            ));
            assert!(matches!(
                client
                    .send_custom_signal(tenant, id, "signal", Some(b"bad"))
                    .await,
                Err(RuntimeError::InstanceNotFound(_))
            ));
            assert!(matches!(
                client.pause_instance(tenant, id).await,
                Err(RuntimeError::InstanceNotFound(_))
            ));
            assert!(matches!(
                client.resume_instance(tenant, id).await,
                Err(RuntimeError::InstanceNotFound(_))
            ));
            assert!(matches!(
                client.stop_instance(tenant, id).await,
                Err(RuntimeError::InstanceNotFound(_))
            ));
        }
        client
            .send_custom_signal(
                tenant,
                &ids[own],
                "signal",
                Some(tenant.as_str().as_bytes()),
            )
            .await
            .unwrap();
        assert_eq!(
            store
                .get_custom_signal(tenant, &ids[own], "signal")
                .await
                .unwrap()
                .unwrap()
                .payload
                .as_deref(),
            Some(tenant.as_str().as_bytes())
        );
        // An options field cannot override the explicitly supplied tenant.
        assert!(
            environment
                .list_instances(
                    tenant,
                    ListInstancesOptions::new().with_tenant_id(tenants[1 - own].as_str())
                )
                .await
                .is_err()
        );
    }
    for (tenant, id) in tenants.iter().zip(&ids) {
        // A parked instance needs no live runner handle for cooperative stop.
        sqlx::query(
            "UPDATE instances SET status = 'suspended' WHERE tenant_id = $1 AND instance_id = $2",
        )
        .bind(tenant.as_str())
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
        client.stop_instance(tenant, id).await.unwrap();
    }
}
