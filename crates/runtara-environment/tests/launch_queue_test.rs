// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Database tests for durable launch queue state transitions.

mod common;

use std::{sync::Arc, time::Duration};

use common::TestContext;
use runtara_core::persistence::PostgresPersistence;
use runtara_environment::runner::{MockRunner, Runner, RunnerHandle};
use runtara_environment::{
    db,
    launch_dispatcher::LaunchDispatcher,
    launch_queue::{
        CancelOutcome, EnqueueOutcome, EnqueueRequest, InitialLaunchOutcome, InitialLaunchRequest,
        LAUNCH_QUEUE_TIMEOUT, LaunchKind, LaunchQueueError, LaunchRepository, LaunchState,
    },
};
use sqlx::PgPool;
use uuid::Uuid;

struct LaunchFixture {
    tenant_id: String,
    instance_id: String,
    image_id: String,
}

async fn fixture(context: &TestContext) -> LaunchFixture {
    let tenant_id = format!("launch-queue-{}", Uuid::new_v4());
    context.cleanup_tenant(&tenant_id).await;

    let image_id = context
        .create_test_image(&tenant_id, "durable-launch")
        .await
        .to_string();
    let instance_id = Uuid::new_v4().to_string();
    assert!(
        db::claim_instance_with_image(
            &context.pool,
            &instance_id,
            &image_id,
            &tenant_id,
            None,
            None,
            None,
        )
        .await
        .expect("instance/image claim must succeed"),
    );

    LaunchFixture {
        tenant_id,
        instance_id,
        image_id,
    }
}

fn request(
    fixture: &LaunchFixture,
    launch_id: impl Into<String>,
    kind: LaunchKind,
    queue_timeout: Duration,
) -> EnqueueRequest {
    EnqueueRequest::immediate(
        launch_id,
        &fixture.instance_id,
        &fixture.tenant_id,
        &fixture.image_id,
        kind,
        queue_timeout,
    )
}

async fn set_instance_status(pool: &PgPool, instance_id: &str, status: &str) {
    sqlx::query(
        r#"
        UPDATE instances
        SET status = $2::instance_status,
            finished_at = NULL,
            termination_reason = NULL
        WHERE instance_id = $1
        "#,
    )
    .bind(instance_id)
    .bind(status)
    .execute(pool)
    .await
    .expect("instance status update must succeed");
}

async fn instance_result(
    pool: &PgPool,
    instance_id: &str,
) -> (String, Option<String>, Option<String>) {
    sqlx::query_as(
        r#"
        SELECT status::TEXT, termination_reason::TEXT, error
        FROM instances
        WHERE instance_id = $1
        "#,
    )
    .bind(instance_id)
    .fetch_one(pool)
    .await
    .expect("instance must exist")
}

#[tokio::test]
async fn launch_is_idempotent_and_parking_releases_the_active_generation() {
    let context = TestContext::new().await.expect("test database must start");
    let fixture = fixture(&context).await;
    let repository = LaunchRepository::new(context.pool.clone());
    let first_id = Uuid::new_v4().to_string();
    let competing_id = Uuid::new_v4().to_string();
    let barrier = Arc::new(tokio::sync::Barrier::new(3));
    let first_task = {
        let repository = repository.clone();
        let request = request(
            &fixture,
            &first_id,
            LaunchKind::Start,
            Duration::from_secs(60),
        );
        let barrier = barrier.clone();
        tokio::spawn(async move {
            barrier.wait().await;
            repository.enqueue(request).await
        })
    };
    let competing_task = {
        let repository = repository.clone();
        let request = request(
            &fixture,
            &competing_id,
            LaunchKind::Start,
            Duration::from_secs(60),
        );
        let barrier = barrier.clone();
        tokio::spawn(async move {
            barrier.wait().await;
            repository.enqueue(request).await
        })
    };
    barrier.wait().await;

    let first = match (
        first_task.await.expect("first enqueue task must not panic"),
        competing_task
            .await
            .expect("competing enqueue task must not panic"),
    ) {
        (Ok(EnqueueOutcome::Enqueued(launch)), Ok(EnqueueOutcome::Existing(existing)))
        | (Ok(EnqueueOutcome::Existing(existing)), Ok(EnqueueOutcome::Enqueued(launch))) => {
            assert_eq!(existing.launch_id, launch.launch_id);
            launch
        }
        outcomes => panic!("concurrent enqueues must converge, got {outcomes:?}"),
    };
    assert!(first.launch_id == first_id || first.launch_id == competing_id);
    assert_eq!(first.state, LaunchState::Queued);

    let duplicate = repository
        .enqueue(request(
            &fixture,
            &first.launch_id,
            LaunchKind::Start,
            Duration::from_secs(60),
        ))
        .await
        .expect("idempotent enqueue must succeed");
    assert!(matches!(
        duplicate,
        EnqueueOutcome::Existing(ref launch) if launch.launch_id == first.launch_id
    ));

    let claimed = repository
        .claim_ready("dispatcher-a", Duration::from_secs(30), 1)
        .await
        .expect("claim must succeed");
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].state, LaunchState::Leased);
    assert_eq!(claimed[0].attempt_count, 1);
    assert!(
        repository
            .begin_start(&first.launch_id, "dispatcher-a")
            .await
            .expect("start transition must succeed")
            .is_some()
    );
    set_instance_status(&context.pool, &fixture.instance_id, "running").await;
    assert!(
        repository
            .mark_running(&first.launch_id, "dispatcher-a")
            .await
            .expect("running transition must succeed")
            .is_some()
    );
    assert!(matches!(
        repository
            .mark_terminal(&first.launch_id, LaunchState::Suspended, None)
            .await,
        Err(runtara_environment::launch_queue::LaunchQueueError::SuspensionRequiresParkingTransaction)
    ));

    let parked = repository
        .mark_suspended(&first.launch_id)
        .await
        .expect("parking transition must succeed")
        .expect("running generation must be parked");
    assert_eq!(parked.state, LaunchState::Suspended);
    assert_eq!(
        instance_result(&context.pool, &fixture.instance_id).await.0,
        "suspended"
    );

    let resumed = repository
        .enqueue(request(
            &fixture,
            Uuid::new_v4().to_string(),
            LaunchKind::Resume,
            Duration::from_secs(60),
        ))
        .await
        .expect("parked instance must accept its next generation");
    assert!(matches!(resumed, EnqueueOutcome::Enqueued(_)));

    context.cleanup().await;
}

#[tokio::test]
async fn expired_dispatcher_leases_are_recovered_once_and_reclaimed() {
    let context = TestContext::new().await.expect("test database must start");
    let fixture = fixture(&context).await;
    let repository = LaunchRepository::new(context.pool.clone());
    let launch_id = Uuid::new_v4().to_string();

    repository
        .enqueue(request(
            &fixture,
            &launch_id,
            LaunchKind::Start,
            Duration::from_secs(60),
        ))
        .await
        .expect("enqueue must succeed");
    let claimed = repository
        .claim_ready("dispatcher-a", Duration::from_secs(60), 1)
        .await
        .expect("claim must succeed");
    assert_eq!(claimed[0].state, LaunchState::Leased);

    sqlx::query(
        "UPDATE instance_launches SET lease_expires_at = NOW() - INTERVAL '1 second' WHERE launch_id = $1",
    )
    .bind(&launch_id)
    .execute(&context.pool)
    .await
    .expect("test lease expiry must be writable");

    let recovered = repository
        .recover_expired_leases(1)
        .await
        .expect("expired lease recovery must succeed");
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].state, LaunchState::Queued);
    assert_eq!(recovered[0].attempt_count, 1);
    assert!(recovered[0].lease_owner.is_none());

    let reclaimed = repository
        .claim_ready("dispatcher-b", Duration::from_secs(60), 1)
        .await
        .expect("reclaim must succeed");
    assert_eq!(reclaimed.len(), 1);
    assert_eq!(reclaimed[0].state, LaunchState::Leased);
    assert_eq!(reclaimed[0].lease_owner.as_deref(), Some("dispatcher-b"));
    assert_eq!(reclaimed[0].attempt_count, 2);
    assert!(
        repository
            .recover_expired_leases(1)
            .await
            .expect("a live lease scan must succeed")
            .is_empty()
    );

    context.cleanup().await;
}

#[tokio::test]
async fn expiry_and_pre_start_cancellation_terminalize_the_matching_instance() {
    let context = TestContext::new().await.expect("test database must start");
    let expiry_fixture = fixture(&context).await;
    let repository = LaunchRepository::new(context.pool.clone());
    let expiry_id = Uuid::new_v4().to_string();

    repository
        .enqueue(request(
            &expiry_fixture,
            &expiry_id,
            LaunchKind::Start,
            Duration::ZERO,
        ))
        .await
        .expect("enqueue must succeed");
    let expired = repository.expire_due(1).await.expect("expiry must succeed");
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].kind, LaunchKind::Start);
    assert_eq!(expired[0].state, LaunchState::Failed);
    assert_eq!(expired[0].last_error.as_deref(), Some(LAUNCH_QUEUE_TIMEOUT));
    assert_eq!(
        instance_result(&context.pool, &expiry_fixture.instance_id).await,
        (
            "failed".to_string(),
            Some(LAUNCH_QUEUE_TIMEOUT.to_string()),
            Some(LAUNCH_QUEUE_TIMEOUT.to_string()),
        )
    );
    assert!(
        repository
            .expire_due(1)
            .await
            .expect("repeat expiry must be idempotent")
            .is_empty()
    );

    let cancel_fixture = fixture(&context).await;
    set_instance_status(&context.pool, &cancel_fixture.instance_id, "suspended").await;
    let cancel_id = Uuid::new_v4().to_string();
    repository
        .enqueue(request(
            &cancel_fixture,
            &cancel_id,
            LaunchKind::Wake,
            Duration::from_secs(60),
        ))
        .await
        .expect("enqueue must succeed");
    repository
        .claim_ready("dispatcher-a", Duration::from_secs(60), 1)
        .await
        .expect("claim must succeed");

    let cancelled = repository
        .cancel_before_start(&cancel_id)
        .await
        .expect("leased launch cancellation must succeed");
    assert!(matches!(
        cancelled,
        CancelOutcome::Cancelled(ref launch)
            if launch.kind == LaunchKind::Wake && launch.state == LaunchState::Cancelled
    ));
    assert_eq!(
        instance_result(&context.pool, &cancel_fixture.instance_id).await,
        ("cancelled".to_string(), Some("cancelled".to_string()), None)
    );
    assert!(matches!(
        repository
            .cancel_before_start(&cancel_id)
            .await
            .expect("repeat cancellation must be idempotent"),
        CancelOutcome::Cancelled(ref launch)
            if launch.kind == LaunchKind::Wake && launch.state == LaunchState::Cancelled
    ));

    context.cleanup().await;
}

#[tokio::test]
async fn enqueue_requires_the_bound_image_and_matching_pre_launch_state() {
    let context = TestContext::new().await.expect("test database must start");
    let fixture = fixture(&context).await;
    let repository = LaunchRepository::new(context.pool.clone());
    let wrong_image = context
        .create_test_image(&fixture.tenant_id, "unbound-image")
        .await
        .to_string();

    let wrong_image_request = EnqueueRequest::immediate(
        Uuid::new_v4().to_string(),
        &fixture.instance_id,
        &fixture.tenant_id,
        wrong_image,
        LaunchKind::Start,
        Duration::from_secs(60),
    );
    assert!(matches!(
        repository.enqueue(wrong_image_request).await,
        Err(runtara_environment::launch_queue::LaunchQueueError::InvalidLaunchTarget { .. })
    ));
    assert!(matches!(
        repository
            .enqueue(request(
                &fixture,
                Uuid::new_v4().to_string(),
                LaunchKind::Resume,
                Duration::from_secs(60),
            ))
            .await,
        Err(runtara_environment::launch_queue::LaunchQueueError::InvalidLaunchTarget { .. })
    ));

    context.cleanup().await;
}

#[tokio::test]
async fn initial_claim_never_commits_a_pending_instance_without_its_launch() {
    let context = TestContext::new().await.expect("test database must start");
    let tenant_id = format!("initial-launch-{}", Uuid::new_v4());
    let image_id = context
        .create_test_image(&tenant_id, "initial-launch")
        .await
        .to_string();
    let repository = LaunchRepository::new(context.pool.clone());
    let instance_id = Uuid::new_v4().to_string();
    let launch_id = Uuid::new_v4().to_string();
    let request = InitialLaunchRequest {
        launch: EnqueueRequest::immediate(
            &launch_id,
            &instance_id,
            &tenant_id,
            &image_id,
            LaunchKind::Start,
            Duration::from_secs(60),
        ),
        input: Some(br#"{"amount":42}"#.to_vec()),
        env: Some(
            [("MODE".to_string(), "test".to_string())]
                .into_iter()
                .collect(),
        ),
        timeout_seconds: Some(30),
    };

    let queued = repository
        .claim_initial(request.clone())
        .await
        .expect("initial claim must commit");
    assert!(matches!(
        queued,
        InitialLaunchOutcome::Enqueued(ref launch)
            if launch.launch_id == launch_id && launch.state == LaunchState::Queued
    ));
    assert_eq!(
        instance_result(&context.pool, &instance_id).await.0,
        "pending",
        "the queue row and Core instance must be visible together"
    );
    assert_eq!(
        db::get_instance_image_id(&context.pool, &instance_id)
            .await
            .expect("image binding read must succeed"),
        Some(image_id.clone())
    );

    let replay = repository
        .claim_initial(request)
        .await
        .expect("idempotent initial claim must succeed");
    assert!(matches!(
        replay,
        InitialLaunchOutcome::ExistingLaunch(ref launch) if launch.launch_id == launch_id
    ));

    let invalid_instance = Uuid::new_v4().to_string();
    let invalid = InitialLaunchRequest {
        launch: EnqueueRequest::immediate(
            Uuid::new_v4().to_string(),
            &invalid_instance,
            &tenant_id,
            "missing-image",
            LaunchKind::Start,
            Duration::from_secs(60),
        ),
        input: None,
        env: None,
        timeout_seconds: Some(30),
    };
    assert!(matches!(
        repository.claim_initial(invalid).await,
        Err(LaunchQueueError::InvalidLaunchTarget { .. })
    ));
    let missing: Option<(String,)> =
        sqlx::query_as("SELECT instance_id FROM instances WHERE instance_id = $1")
            .bind(&invalid_instance)
            .fetch_optional(&context.pool)
            .await
            .expect("invalid initial claim lookup must succeed");
    assert!(
        missing.is_none(),
        "a rejected image must roll back the pending instance insertion"
    );

    context.cleanup().await;
}

#[tokio::test]
async fn dispatcher_hands_off_a_durable_row_without_a_runner_waiter() {
    let context = TestContext::new().await.expect("test database must start");
    let fixture = fixture(&context).await;
    // TestContext's image points at this path; a mock runner accepts any file,
    // while dispatcher preflight still verifies that the durable artifact exists.
    std::fs::write(context.data_dir.join("test_binary"), b"mock workflow")
        .expect("test artifact must be writable");
    let repository = LaunchRepository::new(context.pool.clone());
    let launch_id = Uuid::new_v4().to_string();
    repository
        .enqueue(request(
            &fixture,
            &launch_id,
            LaunchKind::Start,
            Duration::from_secs(60),
        ))
        .await
        .expect("queue row must be inserted");

    let runner = Arc::new(MockRunner::never_completing());
    let dispatcher = LaunchDispatcher::new(
        context.pool.clone(),
        Arc::new(PostgresPersistence::new(context.pool.clone())),
        runner.clone(),
        Arc::new(tokio::sync::Notify::new()),
        Default::default(),
    );
    assert_eq!(
        dispatcher
            .dispatch_once()
            .await
            .expect("dispatch scan must succeed"),
        1
    );
    assert_eq!(
        runner.launch_count(),
        1,
        "one durable generation was handed off"
    );
    assert_eq!(
        repository
            .get(&launch_id)
            .await
            .expect("launch read must succeed")
            .expect("launch must exist")
            .state,
        LaunchState::Running
    );

    // End the detached mock so the background monitor cannot outlive this
    // test. The database cleanup below removes the generation after the task
    // observes the stop.
    let options = runner
        .last_launch()
        .expect("runner launch must be recorded");
    runner
        .stop(&RunnerHandle {
            launch_id: options.launch_id.clone(),
            handle_id: format!("mock_{}", &options.launch_id[..8]),
            instance_id: options.instance_id,
            tenant_id: options.tenant_id,
            started_at: chrono::Utc::now(),
            metrics: None,
        })
        .await
        .expect("mock runner stop must succeed");

    context.cleanup().await;
}
