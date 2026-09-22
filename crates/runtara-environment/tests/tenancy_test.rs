// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Known foreign identifiers must never confer access to environment resources.
mod common;

use common::TestContext;
use runtara_core::{TenantId, persistence::Persistence};
use runtara_environment::{
    container_registry::{ContainerInfo, ContainerRegistry},
    image_registry::{ImageBuilder, ImageFilter, ImageRegistry},
    instance_repository::{InstanceRepository, ListInstancesOptions},
    launch_queue::{
        CancelOutcome, EnqueueRequest, InitialLaunchOutcome, InitialLaunchRequest, LaunchKind,
        LaunchRepository, LaunchState,
    },
};
use runtara_store_postgres::PostgresPersistence;
use std::time::Duration;
use uuid::Uuid;

fn tenant() -> TenantId {
    TenantId::new(Uuid::new_v4().to_string()).unwrap()
}

async fn launch(
    context: &TestContext,
    tenant: &TenantId,
) -> runtara_environment::launch_queue::Launch {
    let image = context
        .create_test_image(tenant.as_str(), "tenancy-fixture")
        .await
        .to_string();
    let request = InitialLaunchRequest {
        launch: EnqueueRequest::immediate(
            Uuid::new_v4().to_string(),
            Uuid::new_v4().to_string(),
            tenant.as_str(),
            image,
            LaunchKind::Start,
            Duration::from_secs(60),
        ),
        input: Some(b"private input".to_vec()),
        env: None,
        timeout_seconds: None,
    };
    match LaunchRepository::new(context.pool.clone())
        .claim_initial(tenant, request)
        .await
        .unwrap()
    {
        InitialLaunchOutcome::Enqueued(launch) => launch,
        outcome => panic!("unexpected initial outcome: {outcome:?}"),
    }
}

#[tokio::test]
async fn instance_reads_counts_and_diagnostics_are_scoped() {
    let context = TestContext::new().await.unwrap();
    let a = tenant();
    let b = tenant();
    let own = launch(&context, &a).await;
    let foreign = launch(&context, &b).await;
    let instances = InstanceRepository::new(context.pool.clone());
    assert!(
        instances
            .detail(&a, &foreign.instance_id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(instances.detail(&a, "missing").await.unwrap().is_none());
    assert!(
        instances
            .image_binding(&a, &foreign.instance_id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        instances
            .completion_metrics(&a, &foreign.instance_id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        instances
            .record_resources_returning_status(&a, &foreign.instance_id, Some(999), Some(9))
            .await
            .unwrap()
            .is_none()
    );
    instances
        .record_stderr(&a, &foreign.instance_id, "foreign write")
        .await
        .unwrap();
    let untouched = instances
        .detail(&b, &foreign.instance_id)
        .await
        .unwrap()
        .unwrap();
    assert!(untouched.stderr.is_none());
    assert!(untouched.memory_peak_bytes.is_none());
    instances
        .record_stderr(&a, &own.instance_id, "owned diagnostic")
        .await
        .unwrap();
    assert!(
        instances
            .record_resources_returning_status(&a, &own.instance_id, Some(123), Some(4))
            .await
            .unwrap()
            .is_some()
    );
    let page = instances
        .list(
            &a,
            &ListInstancesOptions {
                limit: 1,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(page.total_count, 1);
    assert_eq!(page.instances[0].instance_id, own.instance_id);
    assert_eq!(
        instances
            .count_by_status(&a, &["pending".into()], 10)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        instances
            .detail(&a, &own.instance_id)
            .await
            .unwrap()
            .unwrap()
            .stderr
            .as_deref(),
        Some("owned diagnostic")
    );
}

#[tokio::test]
async fn images_are_scoped_even_after_another_tenant_reads_the_same_id() {
    let context = TestContext::new().await.unwrap();
    let a = tenant();
    let b = tenant();
    let registry = ImageRegistry::new(context.pool.clone());
    let file = tempfile::NamedTempFile::new().unwrap();
    let image = ImageBuilder::new(b.as_str(), "same-name", file.path().to_string_lossy()).build();
    registry.register(&b, &image).await.unwrap();
    assert!(registry.get(&b, &image.image_id).await.unwrap().is_some());
    assert!(registry.get(&a, &image.image_id).await.unwrap().is_none());
    assert!(
        !registry
            .artifact_present(&a, &image.image_id)
            .await
            .unwrap()
    );
    assert!(!registry.delete(&a, &image.image_id).await.unwrap());
    assert!(registry.register(&a, &image).await.is_err());
    let own = ImageBuilder::new(a.as_str(), "same-name", file.path().to_string_lossy()).build();
    registry.register(&a, &own).await.unwrap();
    assert_eq!(
        registry
            .get_by_name(&a, "same-name")
            .await
            .unwrap()
            .unwrap()
            .image_id,
        own.image_id
    );
    assert_eq!(
        registry
            .list_filtered(
                &a,
                &ImageFilter {
                    limit: 10,
                    ..Default::default()
                }
            )
            .await
            .unwrap()
            .len(),
        1
    );
    let collision = ImageBuilder::new(a.as_str(), "different-name", "/unused")
        .image_id(&image.image_id)
        .build();
    let error = registry
        .register(&a, &collision)
        .await
        .unwrap_err()
        .to_string();
    assert!(!error.contains(b.as_str()));
    assert!(!error.contains(&image.binary_path));
    assert!(
        registry
            .artifact_present(&b, &image.image_id)
            .await
            .unwrap()
    );
    assert!(registry.delete(&a, &own.image_id).await.unwrap());
    assert!(file.path().exists());
}

#[tokio::test]
async fn launch_replays_and_every_execution_transition_hide_foreign_generations() {
    let context = TestContext::new().await.unwrap();
    let a = tenant();
    let b = tenant();
    let foreign = launch(&context, &b).await;
    let queue = LaunchRepository::new(context.pool.clone());
    assert!(queue.get(&a, &foreign.launch_id).await.unwrap().is_none());
    assert!(
        queue
            .get_active_for_instance(&a, &foreign.instance_id)
            .await
            .unwrap()
            .is_none()
    );
    let replay = EnqueueRequest::immediate(
        &foreign.launch_id,
        &foreign.instance_id,
        a.as_str(),
        &foreign.image_id,
        LaunchKind::Start,
        Duration::from_secs(60),
    );
    assert!(queue.enqueue(&a, replay).await.is_err());
    let own_image = context.create_test_image(a.as_str(), "replay-image").await;
    let initial_replay = InitialLaunchRequest {
        launch: EnqueueRequest::immediate(
            &foreign.launch_id,
            &foreign.instance_id,
            a.as_str(),
            own_image.to_string(),
            LaunchKind::Start,
            Duration::from_secs(60),
        ),
        input: Some(b"must not replace foreign input".to_vec()),
        env: None,
        timeout_seconds: None,
    };
    assert!(matches!(
        queue.claim_initial(&a, initial_replay).await.unwrap(),
        InitialLaunchOutcome::ExistingInstance
    ));
    assert_eq!(
        queue
            .get(&b, &foreign.launch_id)
            .await
            .unwrap()
            .unwrap()
            .tenant_id,
        b.as_str()
    );

    assert!(matches!(
        queue
            .cancel_before_start(&a, &foreign.launch_id)
            .await
            .unwrap(),
        CancelOutcome::NotFound
    ));
    assert!(
        queue
            .claim_ready(&a, "same-owner", Duration::from_secs(60), 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        queue
            .claim_ready_for_preparation(&a, "same-owner", Duration::from_secs(60), 10)
            .await
            .unwrap()
            .is_empty()
    );
    let claim = queue
        .claim_ready_for_preparation(&b, "same-owner", Duration::from_secs(60), 1)
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert!(
        queue
            .promote_prepared(
                &a,
                &claim.launch_id,
                "same-owner",
                claim.attempt_count,
                Duration::from_secs(60)
            )
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        queue
            .requeue_owned(
                &a,
                &claim.launch_id,
                "same-owner",
                claim.attempt_count,
                Duration::ZERO,
                None
            )
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        queue
            .fail_before_runner(
                &a,
                &claim.launch_id,
                "same-owner",
                claim.attempt_count,
                "foreign failure"
            )
            .await
            .unwrap()
            .is_none()
    );
    queue
        .promote_prepared(
            &b,
            &claim.launch_id,
            "same-owner",
            claim.attempt_count,
            Duration::from_secs(60),
        )
        .await
        .unwrap()
        .unwrap();
    assert!(
        queue
            .begin_start(&a, &claim.launch_id, "same-owner", claim.attempt_count)
            .await
            .unwrap()
            .is_none()
    );
    queue
        .begin_start(&b, &claim.launch_id, "same-owner", claim.attempt_count)
        .await
        .unwrap()
        .unwrap();
    assert!(
        queue
            .mark_running(&a, &claim.launch_id, "same-owner", claim.attempt_count)
            .await
            .unwrap()
            .is_none()
    );
    queue
        .mark_running(&b, &claim.launch_id, "same-owner", claim.attempt_count)
        .await
        .unwrap()
        .unwrap();
    assert!(
        queue
            .fail_unconfirmed_running(&a, &claim.launch_id, claim.attempt_count, "foreign failure")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        queue
            .confirm_gate_open(&a, &claim.launch_id, claim.attempt_count)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        !queue
            .is_gate_confirmed(&a, &claim.launch_id, claim.attempt_count)
            .await
            .unwrap()
    );
    assert!(
        queue
            .mark_suspended(&a, &claim.launch_id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        queue
            .mark_terminal(&a, &claim.launch_id, LaunchState::Failed, None)
            .await
            .unwrap()
            .is_none()
    );
    queue
        .confirm_gate_open(&b, &claim.launch_id, claim.attempt_count)
        .await
        .unwrap()
        .unwrap();
    assert!(
        queue
            .is_gate_confirmed(&b, &claim.launch_id, claim.attempt_count)
            .await
            .unwrap()
    );
    assert_eq!(
        queue
            .get(&b, &claim.launch_id)
            .await
            .unwrap()
            .unwrap()
            .state,
        LaunchState::Running
    );
}

#[tokio::test]
async fn queue_batch_limits_expiry_and_recovery_apply_after_tenant_filtering() {
    let context = TestContext::new().await.unwrap();
    let a = tenant();
    let b = tenant();
    let foreign = launch(&context, &b).await;
    let own = launch(&context, &a).await;
    let queue = LaunchRepository::new(context.pool.clone());
    let claimed = queue
        .claim_ready(&a, "owner", Duration::from_secs(60), 1)
        .await
        .unwrap();
    assert_eq!(claimed[0].launch_id, own.launch_id);
    queue
        .claim_ready(&b, "owner", Duration::from_secs(60), 1)
        .await
        .unwrap();
    sqlx::query("UPDATE instance_launches SET lease_expires_at = NOW() - INTERVAL '1 second' WHERE launch_id = ANY($1)")
        .bind(vec![&own.launch_id, &foreign.launch_id]).execute(&context.pool).await.unwrap();
    let recovered = queue.recover_expired_leases(&a, 1).await.unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].launch_id, own.launch_id);
    assert_eq!(
        queue
            .get(&b, &foreign.launch_id)
            .await
            .unwrap()
            .unwrap()
            .state,
        LaunchState::Leased
    );
    sqlx::query("UPDATE instance_launches SET deadline_at = NOW() - INTERVAL '1 second' WHERE launch_id = ANY($1)")
        .bind(vec![&own.launch_id, &foreign.launch_id]).execute(&context.pool).await.unwrap();
    let expired = queue.expire_due(&a, 1).await.unwrap();
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].launch_id, own.launch_id);
    assert_eq!(
        queue
            .get(&b, &foreign.launch_id)
            .await
            .unwrap()
            .unwrap()
            .state,
        LaunchState::Leased
    );
    assert_eq!(
        PostgresPersistence::new(context.pool.clone())
            .get_instance_meta(&b, &foreign.instance_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        runtara_core::domain::InstanceStatus::Pending
    );
}

#[tokio::test]
async fn container_control_and_registration_cannot_cross_tenants() {
    let context = TestContext::new().await.unwrap();
    let a = tenant();
    let b = tenant();
    let launch = launch(&context, &b).await;
    let registry = ContainerRegistry::new(context.pool.clone());
    let info = ContainerInfo {
        container_id: Uuid::new_v4().to_string(),
        launch_id: launch.launch_id.clone(),
        instance_id: launch.instance_id.clone(),
        tenant_id: b.as_str().into(),
        binary_path: "/fixture".into(),
        started_at: chrono::Utc::now(),
        timeout_seconds: None,
    };
    registry.register(&b, &info).await.unwrap();
    assert!(registry.get(&a, &info.instance_id).await.unwrap().is_none());
    assert!(registry.list_registered(&a).await.unwrap().is_empty());
    assert!(registry.tracked_instance_ids(&a).await.unwrap().is_empty());
    assert!(
        registry
            .expired_running_owners(&a)
            .await
            .unwrap()
            .is_empty()
    );
    registry.cleanup(&a, &info.instance_id).await.unwrap();
    assert!(
        !registry
            .cleanup_generation(&a, &info.instance_id, &info.launch_id)
            .await
            .unwrap()
    );
    assert!(
        !registry
            .cleanup_handle(&a, &info.instance_id, &info.launch_id, &info.container_id)
            .await
            .unwrap()
    );
    assert!(registry.register(&a, &info).await.is_err());
    let mut forged = info.clone();
    forged.tenant_id = a.as_str().into();
    assert!(registry.register(&a, &forged).await.is_err());
    assert!(
        registry
            .request_abort(&a, &info.runner_handle(), tokio::time::Instant::now())
            .await
            .is_err()
    );
    assert!(
        registry
            .abort_is_armed(&a, &info.runner_handle(), chrono::Utc::now())
            .await
            .is_err()
    );
    assert_eq!(
        registry
            .get(&b, &info.instance_id)
            .await
            .unwrap()
            .unwrap()
            .container_id,
        info.container_id
    );
    assert!(
        registry
            .cleanup_handle(&b, &info.instance_id, &info.launch_id, &info.container_id)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn uploaded_artifacts_use_tenant_namespaces_and_reject_payload_authority() {
    use runtara_environment::handlers::{
        EnvironmentHandlerState, StoreImageParams, handle_store_image,
    };
    use std::sync::Arc;
    let context = TestContext::new().await.unwrap();
    let state = EnvironmentHandlerState::new(
        context.pool.clone(),
        Arc::new(PostgresPersistence::new(context.pool.clone())),
        Arc::new(runtara_environment::runner::MockRunner::new()),
        context.data_dir.clone(),
    );
    let a = TenantId::new(format!("../{}", Uuid::new_v4())).unwrap();
    let b = tenant();
    let params = |tenant: &TenantId| StoreImageParams {
        tenant_id: tenant.as_str().into(),
        name: "same-name".into(),
        description: None,
        metadata: None,
    };
    assert!(
        handle_store_image(&state, &a, params(&b), b"forbidden")
            .await
            .is_err()
    );
    let a_id = handle_store_image(&state, &a, params(&a), b"a binary")
        .await
        .unwrap();
    let b_id = handle_store_image(&state, &b, params(&b), b"b binary")
        .await
        .unwrap();
    let registry = ImageRegistry::new(context.pool.clone());
    let a_path = registry.get(&a, &a_id).await.unwrap().unwrap().binary_path;
    let b_path = registry.get(&b, &b_id).await.unwrap().unwrap().binary_path;
    assert_ne!(a_path, b_path);
    assert!(std::path::Path::new(&a_path).starts_with(context.data_dir.join("tenants")));
    assert_eq!(
        handle_store_image(&state, &a, params(&a), b"updated a")
            .await
            .unwrap(),
        a_id
    );
    assert_eq!(tokio::fs::read(&a_path).await.unwrap(), b"updated a");
    assert_eq!(tokio::fs::read(&b_path).await.unwrap(), b"b binary");
    sqlx::query(
        "UPDATE images SET updated_at = NOW() - INTERVAL '10 days' WHERE image_id = ANY($1)",
    )
    .bind(vec![&a_id, &b_id])
    .execute(&context.pool)
    .await
    .unwrap();
    assert_eq!(
        registry
            .delete_stale(&a, chrono::Utc::now() - chrono::Duration::days(1), 1)
            .await
            .unwrap(),
        vec![a_id]
    );
    assert!(registry.get(&b, &b_id).await.unwrap().is_some());
    assert_eq!(tokio::fs::read(&b_path).await.unwrap(), b"b binary");
}
