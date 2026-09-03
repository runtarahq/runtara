// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Database coverage for durable workflow-wide launch leases.

mod common;

use std::{sync::Arc, time::Duration};

use common::TestContext;
use runtara_environment::launch_queue::{
    EnqueueOutcome, EnqueueRequest, InitialLaunchOutcome, InitialLaunchRequest, LaunchKind,
    LaunchRepository, LaunchState,
};
use sqlx::PgPool;
use uuid::Uuid;

struct ScopeFixture {
    tenant_id: String,
    image_id: String,
    workflow_id: String,
}

async fn fixture(context: &TestContext) -> ScopeFixture {
    let tenant_id = format!("workflow-launch-lease-{}", Uuid::new_v4());
    let image_id = context
        .create_test_image(&tenant_id, "workflow-launch-lease")
        .await
        .to_string();

    ScopeFixture {
        tenant_id,
        image_id,
        workflow_id: format!("workflow-{}", Uuid::new_v4()),
    }
}

fn initial_request(
    fixture: &ScopeFixture,
    instance_id: impl Into<String>,
    launch_id: impl Into<String>,
    single_instance: bool,
) -> InitialLaunchRequest {
    InitialLaunchRequest {
        launch: EnqueueRequest::immediate(
            launch_id,
            instance_id,
            &fixture.tenant_id,
            &fixture.image_id,
            LaunchKind::Start,
            Duration::from_secs(60),
        )
        .with_workflow_scope(&fixture.workflow_id, single_instance),
        input: None,
        env: None,
        timeout_seconds: None,
    }
}

async fn claim_initial(
    repository: &LaunchRepository,
    fixture: &ScopeFixture,
    single_instance: bool,
) -> runtara_environment::launch_queue::Launch {
    let instance_id = Uuid::new_v4().to_string();
    let launch_id = Uuid::new_v4().to_string();
    match repository
        .claim_initial(initial_request(
            fixture,
            instance_id,
            launch_id,
            single_instance,
        ))
        .await
        .expect("initial launch claim must succeed")
    {
        InitialLaunchOutcome::Enqueued(launch) => launch,
        outcome => panic!("expected initial launch to enqueue, got {outcome:?}"),
    }
}

async fn mark_running_without_gate_confirmation(
    repository: &LaunchRepository,
    launch_id: &str,
) -> i32 {
    let owner = "workflow-launch-lease-test";
    let claimed = repository
        .claim_ready(owner, Duration::from_secs(60), 16)
        .await
        .expect("launch must be claimable");
    let claimed_launch = claimed
        .iter()
        .find(|launch| launch.launch_id == launch_id)
        .expect("target launch must be claimed");
    assert!(
        repository
            .begin_start(launch_id, owner, claimed_launch.attempt_count)
            .await
            .expect("start transition must succeed")
            .is_some(),
        "claimed launch must enter the start gate"
    );
    let running = repository
        .mark_running(launch_id, owner, claimed_launch.attempt_count)
        .await
        .expect("running transition must succeed")
        .expect("start-gated launch must promote to running");
    running.attempt_count
}

async fn promote_running(repository: &LaunchRepository, launch_id: &str) {
    let attempt_count = mark_running_without_gate_confirmation(repository, launch_id).await;
    assert!(
        repository
            .confirm_gate_open(launch_id, attempt_count)
            .await
            .expect("gate confirmation must succeed")
            .is_some(),
        "running generation must be durably confirmed before guest work"
    );
}

async fn active_scope_count(pool: &PgPool, fixture: &ScopeFixture) -> i64 {
    sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM instance_launches
        WHERE tenant_id = $1
          AND workflow_id = $2
          AND state IN ('queued', 'preparing', 'leased', 'starting', 'running')
        "#,
    )
    .bind(&fixture.tenant_id)
    .bind(&fixture.workflow_id)
    .fetch_one(pool)
    .await
    .expect("active scope count must be readable")
}

async fn termination_reason(pool: &PgPool, instance_id: &str) -> Option<String> {
    sqlx::query_scalar("SELECT termination_reason::TEXT FROM instances WHERE instance_id = $1")
        .bind(instance_id)
        .fetch_one(pool)
        .await
        .expect("instance must exist")
}

#[tokio::test]
async fn concurrent_single_instance_claims_admit_exactly_one_durable_lease() {
    let context = TestContext::new().await.expect("test database must start");
    let fixture = fixture(&context).await;
    let repository = LaunchRepository::new(context.pool.clone());
    let barrier = Arc::new(tokio::sync::Barrier::new(3));

    let first = {
        let repository = repository.clone();
        let barrier = barrier.clone();
        let request = initial_request(
            &fixture,
            Uuid::new_v4().to_string(),
            Uuid::new_v4().to_string(),
            true,
        );
        tokio::spawn(async move {
            barrier.wait().await;
            repository.claim_initial(request).await
        })
    };
    let second = {
        let repository = repository.clone();
        let barrier = barrier.clone();
        let request = initial_request(
            &fixture,
            Uuid::new_v4().to_string(),
            Uuid::new_v4().to_string(),
            true,
        );
        tokio::spawn(async move {
            barrier.wait().await;
            repository.claim_initial(request).await
        })
    };

    barrier.wait().await;
    let outcomes = (
        first.await.expect("first task must not panic"),
        second.await.expect("second task must not panic"),
    );
    let admitted_once = matches!(
        (&outcomes.0, &outcomes.1),
        (
            Ok(InitialLaunchOutcome::Enqueued(_)),
            Ok(InitialLaunchOutcome::SingleInstanceActive)
        ) | (
            Ok(InitialLaunchOutcome::SingleInstanceActive),
            Ok(InitialLaunchOutcome::Enqueued(_))
        )
    );
    assert!(
        admitted_once,
        "one concurrent guarded claim must win the database lease, got {outcomes:?}"
    );
    assert_eq!(active_scope_count(&context.pool, &fixture).await, 1);
    let instances: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM instances WHERE tenant_id = $1")
        .bind(&fixture.tenant_id)
        .fetch_one(&context.pool)
        .await
        .expect("tenant instance count must be readable");
    assert_eq!(
        instances, 1,
        "the losing guarded admission must roll back instead of leaving a pending instance"
    );

    context.cleanup().await;
}

#[tokio::test]
async fn parked_history_is_lease_free_and_preserves_the_sleeping_marker() {
    let context = TestContext::new().await.expect("test database must start");
    let fixture = fixture(&context).await;
    let repository = LaunchRepository::new(context.pool.clone());

    // Several parked approvals may exist for this workflow. Each generation
    // is suspended before the next starts, so only live work can hold the
    // workflow-wide lease.
    for index in 0..3 {
        let launch = claim_initial(&repository, &fixture, true).await;
        promote_running(&repository, &launch.launch_id).await;
        if index == 0 {
            sqlx::query(
                r#"
                UPDATE instances
                SET sleep_until = NOW() + INTERVAL '1 hour',
                    termination_reason = 'sleeping'
                WHERE instance_id = $1
                "#,
            )
            .bind(&launch.instance_id)
            .execute(&context.pool)
            .await
            .expect("test sleeping marker must be writable");
        }
        let parked = repository
            .mark_suspended(&launch.launch_id)
            .await
            .expect("parking transition must succeed")
            .expect("running generation must park");
        assert_eq!(parked.state, LaunchState::Suspended);
        if index == 0 {
            assert_eq!(
                termination_reason(&context.pool, &launch.instance_id)
                    .await
                    .as_deref(),
                Some("sleeping"),
                "parking must not erase the durable delay marker"
            );
        }
    }

    assert_eq!(
        active_scope_count(&context.pool, &fixture).await,
        0,
        "parked generations must not retain the workflow lease"
    );
    let next = claim_initial(&repository, &fixture, true).await;
    assert_eq!(next.state, LaunchState::Queued);

    context.cleanup().await;
}

#[tokio::test]
async fn due_wake_and_new_trigger_compete_for_the_same_durable_scope() {
    let context = TestContext::new().await.expect("test database must start");
    let fixture = fixture(&context).await;
    let repository = LaunchRepository::new(context.pool.clone());

    let parked = claim_initial(&repository, &fixture, true).await;
    promote_running(&repository, &parked.launch_id).await;
    repository
        .mark_suspended(&parked.launch_id)
        .await
        .expect("parking transition must succeed")
        .expect("running generation must park");

    let barrier = Arc::new(tokio::sync::Barrier::new(3));
    let wake = {
        let repository = repository.clone();
        let barrier = barrier.clone();
        let request = EnqueueRequest::immediate(
            Uuid::new_v4().to_string(),
            parked.instance_id.clone(),
            fixture.tenant_id.clone(),
            fixture.image_id.clone(),
            LaunchKind::Wake,
            Duration::from_secs(60),
        );
        tokio::spawn(async move {
            barrier.wait().await;
            repository.enqueue(request).await
        })
    };
    let trigger = {
        let repository = repository.clone();
        let barrier = barrier.clone();
        let request = initial_request(
            &fixture,
            Uuid::new_v4().to_string(),
            Uuid::new_v4().to_string(),
            true,
        );
        tokio::spawn(async move {
            barrier.wait().await;
            repository.claim_initial(request).await
        })
    };

    barrier.wait().await;
    let outcomes = (
        wake.await.expect("wake task must not panic"),
        trigger.await.expect("trigger task must not panic"),
    );
    let admitted_once = matches!(
        (&outcomes.0, &outcomes.1),
        (
            Ok(EnqueueOutcome::Enqueued(_)),
            Ok(InitialLaunchOutcome::SingleInstanceActive)
        ) | (
            Ok(EnqueueOutcome::SingleInstanceActive),
            Ok(InitialLaunchOutcome::Enqueued(_))
        )
    );
    assert!(
        admitted_once,
        "due wake and guarded trigger must admit one durable scope winner, got {outcomes:?}"
    );
    assert_eq!(active_scope_count(&context.pool, &fixture).await, 1);

    context.cleanup().await;
}

#[tokio::test]
async fn reconciler_releases_a_lease_after_monitor_crash() {
    let context = TestContext::new().await.expect("test database must start");
    let fixture = fixture(&context).await;
    let repository = LaunchRepository::new(context.pool.clone());

    let running = claim_initial(&repository, &fixture, true).await;
    promote_running(&repository, &running.launch_id).await;

    // Model a host loss after Core committed a durable park but before its
    // monitor released the matching queue generation.
    sqlx::query(
        r#"
        UPDATE instances
        SET status = 'suspended',
            sleep_until = NOW() + INTERVAL '1 hour',
            termination_reason = 'sleeping'
        WHERE instance_id = $1
        "#,
    )
    .bind(&running.instance_id)
    .execute(&context.pool)
    .await
    .expect("test crash window must be writable");

    let released = repository
        .reconcile_released_instances(16)
        .await
        .expect("bounded reconciliation must succeed");
    assert_eq!(released.len(), 1);
    assert_eq!(released[0].launch_id, running.launch_id);
    assert_eq!(released[0].state, LaunchState::Suspended);
    assert_eq!(active_scope_count(&context.pool, &fixture).await, 0);

    let next = claim_initial(&repository, &fixture, true).await;
    assert_eq!(next.state, LaunchState::Queued);

    context.cleanup().await;
}

#[tokio::test]
async fn unconfirmed_running_gate_expires_and_confirmation_removes_its_marker() {
    let context = TestContext::new().await.expect("test database must start");
    let fixture = fixture(&context).await;
    let repository = LaunchRepository::new(context.pool.clone());

    let unconfirmed = claim_initial(&repository, &fixture, true).await;
    mark_running_without_gate_confirmation(&repository, &unconfirmed.launch_id).await;
    sqlx::query(
        r#"
        UPDATE instance_launches
        SET start_gate_deadline_at = NOW() - INTERVAL '1 second'
        WHERE launch_id = $1
        "#,
    )
    .bind(&unconfirmed.launch_id)
    .execute(&context.pool)
    .await
    .expect("test gate deadline must be writable");

    let expired = repository
        .expire_due(16)
        .await
        .expect("unconfirmed gate expiry must succeed");
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].launch_id, unconfirmed.launch_id);
    assert_eq!(expired[0].state, LaunchState::Failed);
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT status::TEXT FROM instances WHERE instance_id = $1"
        )
        .bind(&unconfirmed.instance_id)
        .fetch_one(&context.pool)
        .await
        .expect("expired instance must remain readable"),
        "failed"
    );
    assert_eq!(active_scope_count(&context.pool, &fixture).await, 0);

    let confirmed = claim_initial(&repository, &fixture, true).await;
    let confirmed_attempt =
        mark_running_without_gate_confirmation(&repository, &confirmed.launch_id).await;
    assert!(
        repository
            .confirm_gate_open(&confirmed.launch_id, confirmed_attempt)
            .await
            .expect("gate confirmation must succeed")
            .is_some()
    );
    let marker: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
        "SELECT start_gate_deadline_at FROM instance_launches WHERE launch_id = $1",
    )
    .bind(&confirmed.launch_id)
    .fetch_one(&context.pool)
    .await
    .expect("confirmed gate marker must be readable");
    assert!(
        marker.is_none(),
        "only an unconfirmed running gate remains eligible for expiry"
    );

    context.cleanup().await;
}
