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
            Some(br#"{}"#),
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
            .begin_start(&first.launch_id, "dispatcher-a", claimed[0].attempt_count)
            .await
            .expect("start transition must succeed")
            .is_some()
    );
    assert!(
        repository
            .mark_running(&first.launch_id, "dispatcher-a", claimed[0].attempt_count)
            .await
            .expect("running transition must succeed")
            .is_some()
    );
    assert_eq!(
        instance_result(&context.pool, &fixture.instance_id).await.0,
        "running",
        "the start gate promotion must update Core in the same durable handoff"
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
    let first_fixture = fixture(&context).await;
    let repository = LaunchRepository::new(context.pool.clone());
    let launch_id = Uuid::new_v4().to_string();

    repository
        .enqueue(request(
            &first_fixture,
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

    // A `starting` row is still behind its closed in-process gate. Once its
    // shared handoff lease expires it is safe to reclaim just like a plain
    // claim; no guest could have crossed into execution under the old owner.
    assert!(
        repository
            .begin_start(&launch_id, "dispatcher-b", reclaimed[0].attempt_count)
            .await
            .expect("starting transition must succeed")
            .is_some()
    );
    sqlx::query(
        "UPDATE instance_launches SET lease_expires_at = NOW() - INTERVAL '1 second' WHERE launch_id = $1",
    )
    .bind(&launch_id)
    .execute(&context.pool)
    .await
    .expect("test start-gate expiry must be writable");
    let recovered_starting = repository
        .recover_expired_leases(1)
        .await
        .expect("expired gated start must be recoverable");
    assert_eq!(recovered_starting.len(), 1);
    assert_eq!(recovered_starting[0].state, LaunchState::Queued);
    assert_eq!(
        instance_result(&context.pool, &first_fixture.instance_id)
            .await
            .0,
        "pending",
        "recovering an unopened gate must not fabricate a Core running state"
    );

    // Rows made by a pre-gate binary carry no marker. Even if their short
    // lease is old, a newly deployed dispatcher must leave them to the legacy
    // recovery/monitor path rather than risk duplicating an already-running
    // guest during a rolling deployment.
    let legacy_fixture = fixture(&context).await;
    let legacy_id = Uuid::new_v4().to_string();
    repository
        .enqueue(request(
            &legacy_fixture,
            &legacy_id,
            LaunchKind::Start,
            Duration::from_secs(60),
        ))
        .await
        .expect("legacy-shaped launch must enqueue");
    let legacy_claimed = repository
        .claim_ready("legacy-dispatcher", Duration::from_secs(60), 2)
        .await
        .expect("legacy-shaped launch must claim");
    let legacy_attempt = legacy_claimed
        .iter()
        .find(|launch| launch.launch_id == legacy_id)
        .expect("legacy-shaped launch must be claimed")
        .attempt_count;
    assert!(
        repository
            .begin_start(&legacy_id, "legacy-dispatcher", legacy_attempt)
            .await
            .expect("test setup start must succeed")
            .is_some()
    );
    sqlx::query(
        r#"
        UPDATE instance_launches
        SET lease_expires_at = NOW() - INTERVAL '1 second',
            start_gate_deadline_at = NULL
        WHERE launch_id = $1
        "#,
    )
    .bind(&legacy_id)
    .execute(&context.pool)
    .await
    .expect("legacy-shaped marker must be removable for rollout test");
    assert!(
        repository
            .recover_expired_leases(1)
            .await
            .expect("rollout-fenced recovery scan must succeed")
            .is_empty(),
        "an unmarked pre-gate start must never be reclaimed as safely unopened"
    );
    assert_eq!(
        repository
            .get(&legacy_id)
            .await
            .expect("legacy-shaped launch read must succeed")
            .expect("legacy-shaped launch must remain")
            .state,
        LaunchState::Starting
    );

    context.cleanup().await;
}

#[tokio::test]
async fn expired_preparation_incarnation_cannot_mutate_a_reclaimed_same_owner_launch() {
    let context = TestContext::new().await.expect("test database must start");
    let fixture = fixture(&context).await;
    let repository = LaunchRepository::new(context.pool.clone());
    let launch_id = Uuid::new_v4().to_string();
    let owner = "preparation-owner";

    repository
        .enqueue(request(
            &fixture,
            &launch_id,
            LaunchKind::Start,
            Duration::from_secs(60),
        ))
        .await
        .expect("launch must enqueue");
    let first = repository
        .claim_ready_for_preparation(owner, Duration::from_secs(60), 1)
        .await
        .expect("preparation claim must succeed")
        .pop()
        .expect("one preparation launch must be claimed");
    assert_eq!(first.state, LaunchState::Preparing);

    sqlx::query(
        "UPDATE instance_launches \
         SET lease_expires_at = clock_timestamp() - INTERVAL '1 second' \
         WHERE launch_id = $1",
    )
    .bind(&launch_id)
    .execute(&context.pool)
    .await
    .expect("test preparation lease must be expirable");

    // An expired compiler may complete after recovery starts. Neither a
    // success, capacity retry, nor hard failure from that old incarnation may
    // mutate the row, even though this dispatcher owner will be reused.
    assert!(
        repository
            .promote_prepared(
                &launch_id,
                owner,
                first.attempt_count,
                Duration::from_secs(30)
            )
            .await
            .expect("stale promotion check must succeed")
            .is_none()
    );
    assert!(
        repository
            .requeue_owned(
                &launch_id,
                owner,
                first.attempt_count,
                Duration::ZERO,
                Some("stale_capacity"),
            )
            .await
            .expect("stale requeue check must succeed")
            .is_none()
    );
    assert!(
        repository
            .fail_before_runner(&launch_id, owner, first.attempt_count, "stale_failure")
            .await
            .expect("stale failure check must succeed")
            .is_none()
    );

    let recovered = repository
        .recover_expired_preparations(Duration::ZERO, 1)
        .await
        .expect("expired preparation must recover");
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].state, LaunchState::Queued);

    let second = repository
        .claim_ready_for_preparation(owner, Duration::from_secs(60), 1)
        .await
        .expect("same owner may reclaim with a fresh incarnation")
        .pop()
        .expect("reclaimed preparation must exist");
    assert!(second.attempt_count > first.attempt_count);
    assert_eq!(second.state, LaunchState::Preparing);

    assert!(
        repository
            .promote_prepared(
                &launch_id,
                owner,
                first.attempt_count,
                Duration::from_secs(30)
            )
            .await
            .expect("old completion after same-owner reclaim must be harmless")
            .is_none()
    );
    assert!(
        repository
            .requeue_owned(
                &launch_id,
                owner,
                first.attempt_count,
                Duration::ZERO,
                Some("stale_capacity"),
            )
            .await
            .expect("old retry after same-owner reclaim must be harmless")
            .is_none()
    );
    assert!(
        repository
            .fail_before_runner(&launch_id, owner, first.attempt_count, "stale_failure")
            .await
            .expect("old failure after same-owner reclaim must be harmless")
            .is_none()
    );
    assert_eq!(
        repository
            .get(&launch_id)
            .await
            .expect("launch must remain readable")
            .expect("launch must remain present")
            .attempt_count,
        second.attempt_count,
        "only the fresh preparation incarnation remains authoritative"
    );

    context.cleanup().await;
}

#[tokio::test]
async fn gate_confirmation_is_fenced_by_attempt_and_real_database_time() {
    let context = TestContext::new().await.expect("test database must start");
    let fixture = fixture(&context).await;
    let repository = LaunchRepository::new(context.pool.clone());
    let launch_id = Uuid::new_v4().to_string();
    let owner = "gate-owner";

    repository
        .enqueue(request(
            &fixture,
            &launch_id,
            LaunchKind::Start,
            Duration::from_secs(60),
        ))
        .await
        .expect("launch must enqueue");
    let claimed = repository
        .claim_ready(owner, Duration::from_secs(60), 1)
        .await
        .expect("launch must claim")
        .pop()
        .expect("one launch must claim");
    repository
        .begin_start(&launch_id, owner, claimed.attempt_count)
        .await
        .expect("start transition must succeed")
        .expect("launch must enter starting");
    let running = repository
        .mark_running(&launch_id, owner, claimed.attempt_count)
        .await
        .expect("running transition must succeed")
        .expect("launch must become running");

    // Model a later recovery incarnation. An old runner must not clear that
    // later attempt's durable marker or terminalize its Core instance.
    let later_attempt = running.attempt_count + 1;
    sqlx::query(
        r#"
        UPDATE instance_launches
        SET attempt_count = $2,
            start_gate_deadline_at = clock_timestamp() + INTERVAL '30 seconds'
        WHERE launch_id = $1
        "#,
    )
    .bind(&launch_id)
    .bind(later_attempt)
    .execute(&context.pool)
    .await
    .expect("test later attempt must be writable");
    assert!(
        repository
            .confirm_gate_open(&launch_id, running.attempt_count)
            .await
            .expect("stale confirmation query must succeed")
            .is_none(),
        "an old runner cannot clear a newer attempt's marker"
    );
    assert!(
        repository
            .fail_unconfirmed_running(
                &launch_id,
                running.attempt_count,
                "stale monitor must not win",
            )
            .await
            .expect("stale terminalization query must succeed")
            .is_none(),
        "an old monitor cannot terminalize a newer attempt"
    );
    let later_marker_is_present: bool = sqlx::query_scalar(
        "SELECT start_gate_deadline_at IS NOT NULL FROM instance_launches WHERE launch_id = $1",
    )
    .bind(&launch_id)
    .fetch_one(&context.pool)
    .await
    .expect("launch must remain readable");
    assert!(
        later_marker_is_present,
        "the later attempt remains recoverably marked"
    );
    assert!(
        repository
            .confirm_gate_open(&launch_id, later_attempt)
            .await
            .expect("current confirmation query must succeed")
            .is_some(),
        "the matching attempt may clear its own marker"
    );

    // Hold the row lock until after its marker deadline. PostgreSQL `NOW()`
    // is transaction-stable, so this regression specifically proves the
    // confirmation predicate uses real `clock_timestamp()` after lock wait.
    sqlx::query(
        "UPDATE instance_launches \
         SET start_gate_deadline_at = clock_timestamp() + INTERVAL '500 milliseconds' \
         WHERE launch_id = $1",
    )
    .bind(&launch_id)
    .execute(&context.pool)
    .await
    .expect("test short gate deadline must be writable");
    let mut transaction = context
        .pool
        .begin()
        .await
        .expect("lock transaction must begin");
    sqlx::query("SELECT launch_id FROM instance_launches WHERE launch_id = $1 FOR UPDATE")
        .bind(&launch_id)
        .execute(&mut *transaction)
        .await
        .expect("test row lock must succeed");
    let delayed_confirmation = {
        let repository = repository.clone();
        let launch_id = launch_id.clone();
        tokio::spawn(async move {
            repository
                .confirm_gate_open(&launch_id, later_attempt)
                .await
        })
    };
    // Do not rely on scheduling the spawned query before the sleep below: it
    // must actually be waiting on this row lock. The test database is single
    // threaded here, and PostgreSQL exposes a blocked UPDATE as a Lock wait.
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let blocked: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) \
                 FROM pg_stat_activity \
                 WHERE datname = current_database() \
                   AND wait_event_type = 'Lock' \
                   AND query LIKE '%UPDATE instance_launches%'",
            )
            .fetch_one(&context.pool)
            .await
            .expect("observe blocked confirmation query");
            if blocked > 0 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("confirmation query must block on the test row lock");
    tokio::time::sleep(Duration::from_millis(600)).await;
    transaction
        .commit()
        .await
        .expect("test row lock must release");
    assert!(
        delayed_confirmation
            .await
            .expect("confirmation task must not panic")
            .expect("confirmation query must succeed")
            .is_none(),
        "a confirmation blocked past its marker deadline must not clear it"
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

    let starting_fixture = fixture(&context).await;
    let starting_id = Uuid::new_v4().to_string();
    repository
        .enqueue(request(
            &starting_fixture,
            &starting_id,
            LaunchKind::Start,
            Duration::from_secs(60),
        ))
        .await
        .expect("starting launch must enqueue");
    let starting_claimed = repository
        .claim_ready("dispatcher-gated", Duration::from_secs(60), 1)
        .await
        .expect("starting launch must claim");
    assert!(
        repository
            .begin_start(
                &starting_id,
                "dispatcher-gated",
                starting_claimed[0].attempt_count,
            )
            .await
            .expect("starting transition must succeed")
            .is_some()
    );
    assert!(matches!(
        repository
            .cancel_before_start(&starting_id)
            .await
            .expect("closed start gate cancellation must succeed"),
        CancelOutcome::Cancelled(ref launch) if launch.state == LaunchState::Cancelled
    ));
    assert_eq!(
        instance_result(&context.pool, &starting_fixture.instance_id)
            .await
            .0,
        "cancelled",
        "cancelling a gated start must terminalize its still-pre-start Core row"
    );

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
    // Preparation deliberately runs in a bounded detached worker so a slow
    // compile cannot stall the dispatcher/reaper loop. Wait for its durable
    // handoff rather than assuming `dispatch_once` synchronously reaches a
    // runner permit.
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let state = repository
                .get(&launch_id)
                .await
                .expect("launch read must succeed")
                .expect("launch must exist")
                .state;
            if runner.launch_count() == 1 && state == LaunchState::Running {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("one durable generation must be handed off without an in-memory waiter");

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
