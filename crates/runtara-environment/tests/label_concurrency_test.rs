// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Concurrency per run label (`labelConcurrency`, `labelRequired`): a run
//! holds its label from its start until it ends, suspended time included,
//! and a start for a held label waits in order, is refused, or (once a run
//! with the label has completed) is refused for good.

mod common;

use std::{sync::Arc, time::Duration};

use common::TestContext;
use runtara_core::persistence::ParentLink;
use runtara_dsl::LabelConcurrency;
use runtara_environment::launch_queue::{
    EnqueueRequest, InitialLaunchOutcome, InitialLaunchRequest, LaunchKind, LaunchRepository,
};
use sqlx::PgPool;
use uuid::Uuid;

struct Fixture {
    tenant_id: String,
    image_id: String,
    workflow_id: String,
}

async fn fixture(context: &TestContext) -> Fixture {
    let tenant_id = format!("label-concurrency-{}", Uuid::new_v4());
    let workflow_id = format!("workflow-{}", Uuid::new_v4());
    // Named as the engine names images, `<workflow>:<version>`, which is
    // how `run_keys` knows a run's workflow.
    let image_id = context
        .create_test_image(&tenant_id, &format!("{workflow_id}:1"))
        .await
        .to_string();
    Fixture {
        tenant_id,
        image_id,
        workflow_id,
    }
}

fn request(
    fixture: &Fixture,
    label: Option<&str>,
    policy: LabelConcurrency,
    required: bool,
) -> InitialLaunchRequest {
    InitialLaunchRequest {
        run_label: label.map(str::to_owned),
        parent: None,
        launch: EnqueueRequest::immediate(
            Uuid::new_v4().to_string(),
            Uuid::new_v4().to_string(),
            &fixture.tenant_id,
            &fixture.image_id,
            LaunchKind::Start,
            Duration::from_secs(60),
        )
        .with_workflow_scope(&fixture.workflow_id, false)
        .with_label_policy(policy, required),
        input: None,
        env: None,
        timeout_seconds: None,
        shared_child_steps: Default::default(),
    }
}

/// Starts a run and returns its id and how its start ended.
async fn start(
    repository: &LaunchRepository,
    fixture: &Fixture,
    label: &str,
    policy: LabelConcurrency,
) -> (String, InitialLaunchOutcome) {
    let request = request(fixture, Some(label), policy, false);
    let id = request.launch.instance_id.clone();
    let outcome = repository.claim_initial(request).await.expect("claim runs");
    (id, outcome)
}

/// The run's status changes as the engine changes it.
async fn set_status(pool: &PgPool, instance_id: &str, status: &str) {
    sqlx::query("UPDATE instances SET status = $2::instance_status WHERE instance_id = $1")
        .bind(instance_id)
        .bind(status)
        .execute(pool)
        .await
        .expect("status update");
}

async fn launch_state(pool: &PgPool, instance_id: &str) -> String {
    sqlx::query_scalar(
        "SELECT state FROM instance_launches WHERE instance_id = $1 \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(instance_id)
    .fetch_one(pool)
    .await
    .expect("launch state")
}

async fn instances(pool: &PgPool, fixture: &Fixture) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM instances WHERE tenant_id = $1")
        .bind(&fixture.tenant_id)
        .fetch_one(pool)
        .await
        .expect("instance count")
}

#[tokio::test]
async fn refuse_turns_a_second_run_away_while_the_first_holds_the_label() {
    let context = TestContext::new().await.expect("test database");
    let fixture = fixture(&context).await;
    let repository = LaunchRepository::new(context.pool.clone());

    let (first, outcome) = start(&repository, &fixture, "order-1", LabelConcurrency::Refuse).await;
    assert!(matches!(outcome, InitialLaunchOutcome::Enqueued(_)));
    let (_, outcome) = start(&repository, &fixture, "order-1", LabelConcurrency::Refuse).await;
    match outcome {
        InitialLaunchOutcome::LabelHeld { label, holder } => {
            assert_eq!(label, "order-1");
            assert_eq!(holder, first);
        }
        other => panic!("expected the label to be held, got {other:?}"),
    }
    // Nothing of the refused start stays behind.
    assert_eq!(instances(&context.pool, &fixture).await, 1);
    // Another label is free.
    let (_, outcome) = start(&repository, &fixture, "order-2", LabelConcurrency::Refuse).await;
    assert!(matches!(outcome, InitialLaunchOutcome::Enqueued(_)));

    // Suspended, it still holds the label; ended, it does not.
    set_status(&context.pool, &first, "suspended").await;
    let (_, outcome) = start(&repository, &fixture, "order-1", LabelConcurrency::Refuse).await;
    assert!(matches!(outcome, InitialLaunchOutcome::LabelHeld { .. }));
    set_status(&context.pool, &first, "failed").await;
    let (_, outcome) = start(&repository, &fixture, "order-1", LabelConcurrency::Refuse).await;
    assert!(matches!(outcome, InitialLaunchOutcome::Enqueued(_)));

    context.cleanup().await;
}

#[tokio::test]
async fn queue_lets_waiting_runs_have_the_label_in_the_order_started() {
    let context = TestContext::new().await.expect("test database");
    let fixture = fixture(&context).await;
    let repository = LaunchRepository::new(context.pool.clone());
    let pool = &context.pool;

    let (a, outcome) = start(&repository, &fixture, "order-1", LabelConcurrency::Queue).await;
    assert!(matches!(outcome, InitialLaunchOutcome::Enqueued(_)));
    let (b, outcome) = start(&repository, &fixture, "order-1", LabelConcurrency::Queue).await;
    assert!(
        matches!(outcome, InitialLaunchOutcome::Waiting(_)),
        "{outcome:?}"
    );
    let (c, outcome) = start(&repository, &fixture, "order-1", LabelConcurrency::Queue).await;
    assert!(
        matches!(outcome, InitialLaunchOutcome::Waiting(_)),
        "{outcome:?}"
    );
    assert_eq!(launch_state(pool, &b).await, "waiting");
    assert_eq!(launch_state(pool, &c).await, "waiting");
    // Each says what it waits for: the label, its holder, and its place.
    let waits = repository
        .label_waits(&[a.clone(), b.clone(), c.clone()])
        .await
        .expect("waits read");
    assert!(!waits.contains_key(&a), "the holder waits for nothing");
    assert_eq!(waits[&b].label, "order-1");
    assert_eq!(waits[&b].holder.as_deref(), Some(a.as_str()));
    assert_eq!(waits[&b].ahead, 0);
    assert_eq!(waits[&c].holder.as_deref(), Some(a.as_str()));
    assert_eq!(waits[&c].ahead, 1);

    // A suspended holder keeps them waiting.
    set_status(pool, &a, "suspended").await;
    assert_eq!(launch_state(pool, &b).await, "waiting");

    // It ends: the first waiting run has the label, the next still waits.
    set_status(pool, &a, "completed").await;
    assert_eq!(launch_state(pool, &b).await, "queued");
    assert_eq!(launch_state(pool, &c).await, "waiting");
    let waits = repository
        .label_waits(&[b.clone(), c.clone()])
        .await
        .unwrap();
    assert!(!waits.contains_key(&b), "b has the label now");
    assert_eq!(waits[&c].holder.as_deref(), Some(b.as_str()));
    assert_eq!(waits[&c].ahead, 0);
    // A start now waits behind them both.
    let (d, outcome) = start(&repository, &fixture, "order-1", LabelConcurrency::Queue).await;
    assert!(matches!(outcome, InitialLaunchOutcome::Waiting(_)));

    set_status(pool, &b, "running").await;
    set_status(pool, &b, "failed").await;
    assert_eq!(launch_state(pool, &c).await, "queued");
    assert_eq!(launch_state(pool, &d).await, "waiting");

    // A waiting run cancelled leaves the queue and moves no one ahead of the
    // holder.
    let cancelled = repository.cancel_waiting(&d).await.expect("cancel runs");
    assert!(cancelled.is_some());
    assert_eq!(launch_state(pool, &d).await, "cancelled");
    let status: String =
        sqlx::query_scalar("SELECT status::text FROM instances WHERE instance_id = $1")
            .bind(&d)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(status, "cancelled");
    assert!(repository.cancel_waiting(&c).await.unwrap().is_none());

    context.cleanup().await;
}

#[tokio::test]
async fn once_refuses_a_label_that_has_completed_but_not_one_that_failed() {
    let context = TestContext::new().await.expect("test database");
    let fixture = fixture(&context).await;
    let repository = LaunchRepository::new(context.pool.clone());
    let pool = &context.pool;

    let (a, _) = start(&repository, &fixture, "invoice-7", LabelConcurrency::Once).await;
    // While it runs, the label is held.
    let (_, outcome) = start(&repository, &fixture, "invoice-7", LabelConcurrency::Once).await;
    assert!(matches!(outcome, InitialLaunchOutcome::LabelHeld { .. }));
    // A failed run does not use the label up.
    set_status(pool, &a, "failed").await;
    let (b, outcome) = start(&repository, &fixture, "invoice-7", LabelConcurrency::Once).await;
    assert!(matches!(outcome, InitialLaunchOutcome::Enqueued(_)));
    // A completed one does, for good.
    set_status(pool, &b, "completed").await;
    let (_, outcome) = start(&repository, &fixture, "invoice-7", LabelConcurrency::Once).await;
    match outcome {
        InitialLaunchOutcome::LabelUsed { label } => assert_eq!(label, "invoice-7"),
        other => panic!("expected the label to be used up, got {other:?}"),
    }
    // Even after its runs are pruned: `run_keys` remembers.
    sqlx::query("DELETE FROM instances WHERE tenant_id = $1")
        .bind(&fixture.tenant_id)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO run_keys (tenant_id, workflow_id, run_label, instance_id, image_name, \
         run_created_at, status) VALUES ($1, $2, 'invoice-8', 'gone', $2 || ':1', NOW(), 'completed')",
    )
    .bind(&fixture.tenant_id)
    .bind(&fixture.workflow_id)
    .execute(pool)
    .await
    .unwrap();
    let (_, outcome) = start(&repository, &fixture, "invoice-8", LabelConcurrency::Once).await;
    assert!(
        matches!(outcome, InitialLaunchOutcome::LabelUsed { .. }),
        "{outcome:?}"
    );

    context.cleanup().await;
}

#[tokio::test]
async fn a_required_label_is_required_and_allow_changes_nothing() {
    let context = TestContext::new().await.expect("test database");
    let fixture = fixture(&context).await;
    let repository = LaunchRepository::new(context.pool.clone());

    let outcome = repository
        .claim_initial(request(&fixture, None, LabelConcurrency::Allow, true))
        .await
        .unwrap();
    assert!(matches!(outcome, InitialLaunchOutcome::LabelRequired));
    assert_eq!(instances(&context.pool, &fixture).await, 0);

    for _ in 0..2 {
        let (_, outcome) = start(&repository, &fixture, "order-1", LabelConcurrency::Allow).await;
        assert!(matches!(outcome, InitialLaunchOutcome::Enqueued(_)));
    }

    context.cleanup().await;
}

#[tokio::test]
async fn a_child_is_refused_its_parents_label_rather_than_waiting_forever() {
    let context = TestContext::new().await.expect("test database");
    let fixture = fixture(&context).await;
    let repository = LaunchRepository::new(context.pool.clone());

    let (parent, _) = start(&repository, &fixture, "order-1", LabelConcurrency::Queue).await;
    let mut child = request(&fixture, Some("order-1"), LabelConcurrency::Queue, false);
    child.parent = Some(ParentLink {
        parent_instance_id: parent.clone(),
        parent_close_policy: "cancel".into(),
        admitted_at: chrono::Utc::now(),
    });
    match repository.claim_initial(child).await.unwrap() {
        InitialLaunchOutcome::LabelHeld { holder, .. } => assert_eq!(holder, parent),
        other => panic!("expected the child to be refused, got {other:?}"),
    }

    context.cleanup().await;
}

#[tokio::test]
async fn concurrent_starts_for_one_label_admit_one() {
    let context = TestContext::new().await.expect("test database");
    let fixture = Arc::new(fixture(&context).await);
    let repository = LaunchRepository::new(context.pool.clone());

    for round in 0..10 {
        let label = format!("order-{round}");
        let barrier = Arc::new(tokio::sync::Barrier::new(4));
        let mut tasks = Vec::new();
        for _ in 0..4 {
            let (repository, fixture, barrier, label) = (
                repository.clone(),
                fixture.clone(),
                barrier.clone(),
                label.clone(),
            );
            tasks.push(tokio::spawn(async move {
                barrier.wait().await;
                repository
                    .claim_initial(request(
                        &fixture,
                        Some(&label),
                        LabelConcurrency::Refuse,
                        false,
                    ))
                    .await
                    .unwrap()
            }));
        }
        let mut admitted = 0;
        for task in tasks {
            match task.await.unwrap() {
                InitialLaunchOutcome::Enqueued(_) => admitted += 1,
                InitialLaunchOutcome::LabelHeld { .. } => {}
                other => panic!("unexpected {other:?}"),
            }
        }
        assert_eq!(admitted, 1, "round {round}");
    }

    context.cleanup().await;
}

/// A start that decides to wait and a holder that ends at the same moment:
/// whichever goes first, the waiting run is never left waiting for a holder
/// that is gone.
#[tokio::test]
async fn a_run_never_waits_for_a_holder_that_has_ended() {
    let context = TestContext::new().await.expect("test database");
    let fixture = Arc::new(fixture(&context).await);
    let repository = LaunchRepository::new(context.pool.clone());
    let pool = context.pool.clone();

    for round in 0..20 {
        let label = format!("order-{round}");
        let (holder, _) = start(&repository, &fixture, &label, LabelConcurrency::Queue).await;
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let ending = {
            let (pool, barrier, holder) = (pool.clone(), barrier.clone(), holder.clone());
            tokio::spawn(async move {
                barrier.wait().await;
                set_status(&pool, &holder, "completed").await;
            })
        };
        let starting = {
            let (repository, fixture, barrier, label) = (
                repository.clone(),
                fixture.clone(),
                barrier.clone(),
                label.clone(),
            );
            tokio::spawn(async move {
                barrier.wait().await;
                start(&repository, &fixture, &label, LabelConcurrency::Queue).await
            })
        };
        ending.await.unwrap();
        let (next, _) = starting.await.unwrap();
        assert_eq!(
            launch_state(&pool, &next).await,
            "queued",
            "round {round}: the next run must not wait for an ended holder"
        );
    }

    context.cleanup().await;
}

/// A run whose launch carries no label (one from before launches recorded
/// it) still holds its label, and still hands it on when it ends: holders
/// are found by the run's own label.
#[tokio::test]
async fn a_holder_from_before_launches_carried_labels_still_holds() {
    let context = TestContext::new().await.expect("test database");
    let fixture = fixture(&context).await;
    let repository = LaunchRepository::new(context.pool.clone());
    let pool = &context.pool;

    let (old, _) = start(&repository, &fixture, "order-1", LabelConcurrency::Queue).await;
    sqlx::query("UPDATE instance_launches SET run_label = NULL WHERE instance_id = $1")
        .bind(&old)
        .execute(pool)
        .await
        .unwrap();

    let (_, outcome) = start(&repository, &fixture, "order-1", LabelConcurrency::Refuse).await;
    assert!(
        matches!(&outcome, InitialLaunchOutcome::LabelHeld { holder, .. } if *holder == old),
        "{outcome:?}"
    );
    let (next, outcome) = start(&repository, &fixture, "order-1", LabelConcurrency::Queue).await;
    assert!(
        matches!(outcome, InitialLaunchOutcome::Waiting(_)),
        "{outcome:?}"
    );
    set_status(pool, &old, "completed").await;
    assert_eq!(launch_state(pool, &next).await, "queued");

    context.cleanup().await;
}

async fn write_state(pool: &PgPool, instance_id: &str, state: serde_json::Value) {
    sqlx::query(
        "INSERT INTO instance_state (instance_id, state) VALUES ($1, $2) \
         ON CONFLICT (instance_id) DO UPDATE SET state = EXCLUDED.state, \
         state_updated_at = clock_timestamp()",
    )
    .bind(instance_id)
    .bind(state)
    .execute(pool)
    .await
    .unwrap();
}

async fn state_of(pool: &PgPool, instance_id: &str) -> Option<serde_json::Value> {
    sqlx::query_scalar("SELECT state FROM instance_state WHERE instance_id = $1")
        .bind(instance_id)
        .fetch_optional(pool)
        .await
        .unwrap()
}

async fn start_carrying(
    repository: &LaunchRepository,
    fixture: &Fixture,
    label: &str,
) -> (String, InitialLaunchOutcome) {
    let mut request = request(fixture, Some(label), LabelConcurrency::Queue, false);
    request.launch.carry_state = true;
    let id = request.launch.instance_id.clone();
    (id, repository.claim_initial(request).await.unwrap())
}

/// `carryStateForward`: each run of a label starts with the state the run
/// before it left: a waiting run when it is let in, a run that starts at
/// once when it is admitted.
#[tokio::test]
async fn each_run_of_a_label_carries_its_predecessors_state_on() {
    let context = TestContext::new().await.expect("test database");
    let fixture = fixture(&context).await;
    let repository = LaunchRepository::new(context.pool.clone());
    let pool = &context.pool;

    // A label with no state yet carries nothing.
    let (a, outcome) = start_carrying(&repository, &fixture, "cursor-1").await;
    assert!(matches!(outcome, InitialLaunchOutcome::Enqueued(_)));
    assert_eq!(state_of(pool, &a).await, None);

    // A writes; B waits behind it, with no state while it waits.
    write_state(pool, &a, serde_json::json!({"cursor": "c-1", "pages": 1})).await;
    let (b, outcome) = start_carrying(&repository, &fixture, "cursor-1").await;
    assert!(matches!(outcome, InitialLaunchOutcome::Waiting(_)));
    assert_eq!(state_of(pool, &b).await, None);
    // A moves on before it ends: B gets what A ended with.
    write_state(pool, &a, serde_json::json!({"cursor": "c-2", "pages": 2})).await;
    set_status(pool, &a, "completed").await;
    assert_eq!(launch_state(pool, &b).await, "queued");
    assert_eq!(
        state_of(pool, &b).await,
        Some(serde_json::json!({"cursor": "c-2", "pages": 2}))
    );

    // B writes and ends; C starts at once with B's state.
    write_state(pool, &b, serde_json::json!({"cursor": "c-3", "pages": 3})).await;
    set_status(pool, &b, "completed").await;
    let (c, outcome) = start_carrying(&repository, &fixture, "cursor-1").await;
    assert!(matches!(outcome, InitialLaunchOutcome::Enqueued(_)));
    assert_eq!(
        state_of(pool, &c).await,
        Some(serde_json::json!({"cursor": "c-3", "pages": 3}))
    );

    // A run that does not carry starts empty.
    set_status(pool, &c, "completed").await;
    let (d, _) = start(&repository, &fixture, "cursor-1", LabelConcurrency::Queue).await;
    assert_eq!(state_of(pool, &d).await, None);

    context.cleanup().await;
}

/// Retrying `claim_initial` with the same instance id while the run waits for
/// its label replays the waiting launch rather than reporting `ExistingInstance`.
#[tokio::test]
async fn idempotent_retry_on_a_waiting_run_returns_existing_launch() {
    let context = TestContext::new().await.expect("test database");
    let fixture = fixture(&context).await;
    let repository = LaunchRepository::new(context.pool.clone());

    let (_holder, _) = start(&repository, &fixture, "order-1", LabelConcurrency::Queue).await;
    let req = request(&fixture, Some("order-1"), LabelConcurrency::Queue, false);
    let waiting_id = req.launch.instance_id.clone();
    assert!(matches!(
        repository.claim_initial(req.clone()).await.unwrap(),
        InitialLaunchOutcome::Waiting(_)
    ));
    match repository.claim_initial(req).await.unwrap() {
        InitialLaunchOutcome::ExistingLaunch(launch) => {
            assert_eq!(launch.instance_id, waiting_id);
            assert_eq!(launch.state.as_str(), "waiting");
        }
        other => panic!("expected ExistingLaunch for waiting retry, got {other:?}"),
    }

    context.cleanup().await;
}

/// A grandchild waiting for its grandparent's label would deadlock its lineage
/// just like a direct child: it is refused instead.
#[tokio::test]
async fn a_grandchild_is_refused_its_grandparents_label() {
    let context = TestContext::new().await.expect("test database");
    let fixture = fixture(&context).await;
    let repository = LaunchRepository::new(context.pool.clone());

    let (grandparent, _) = start(&repository, &fixture, "order-1", LabelConcurrency::Queue).await;
    let mut middle = request(&fixture, None, LabelConcurrency::Allow, false);
    middle.parent = Some(ParentLink {
        parent_instance_id: grandparent.clone(),
        parent_close_policy: "cancel".into(),
        admitted_at: chrono::Utc::now(),
    });
    let middle_id = middle.launch.instance_id.clone();
    assert!(matches!(
        repository.claim_initial(middle).await.unwrap(),
        InitialLaunchOutcome::Enqueued(_)
    ));

    let mut grandchild = request(&fixture, Some("order-1"), LabelConcurrency::Queue, false);
    grandchild.parent = Some(ParentLink {
        parent_instance_id: middle_id,
        parent_close_policy: "cancel".into(),
        admitted_at: chrono::Utc::now(),
    });
    match repository.claim_initial(grandchild).await.unwrap() {
        InitialLaunchOutcome::LabelHeld { holder, .. } => assert_eq!(holder, grandparent),
        other => panic!("expected grandchild refusal naming grandparent, got {other:?}"),
    }

    context.cleanup().await;
}

/// A waiting run does not displace the active holder in `run_keys` while it
/// waits, and takes its place once promoted (or leaves `run_keys` untouched if
/// cancelled while waiting).
#[tokio::test]
async fn a_waiting_run_does_not_overwrite_run_keys_until_promoted() {
    let context = TestContext::new().await.expect("test database");
    let fixture = fixture(&context).await;
    let repository = LaunchRepository::new(context.pool.clone());
    let pool = &context.pool;

    let (holder, _) = start(&repository, &fixture, "order-1", LabelConcurrency::Queue).await;
    let (waiter, outcome) = start(&repository, &fixture, "order-1", LabelConcurrency::Queue).await;
    assert!(matches!(outcome, InitialLaunchOutcome::Waiting(_)));

    let key_run = || async {
        sqlx::query_scalar::<_, String>(
            "SELECT instance_id FROM run_keys \
             WHERE tenant_id = $1 AND workflow_id = $2 AND run_label = 'order-1'",
        )
        .bind(&fixture.tenant_id)
        .bind(&fixture.workflow_id)
        .fetch_one(pool)
        .await
        .unwrap()
    };
    assert_eq!(
        key_run().await,
        holder,
        "waiting run must not steal run_keys"
    );

    // Cancelling the waiter leaves the holder in run_keys.
    assert!(repository.cancel_waiting(&waiter).await.unwrap().is_some());
    assert_eq!(key_run().await, holder);

    // Another waiter promoted when holder finishes becomes the key's latest.
    let (next, _) = start(&repository, &fixture, "order-1", LabelConcurrency::Queue).await;
    assert_eq!(key_run().await, holder);
    set_status(pool, &holder, "completed").await;
    assert_eq!(launch_state(pool, &next).await, "queued");
    assert_eq!(key_run().await, next);

    context.cleanup().await;
}

/// Cancelling a waiting run concurrently with its holder ending never deadlocks.
#[tokio::test]
async fn cancel_waiting_racing_holder_completion_never_deadlocks() {
    let context = TestContext::new().await.expect("test database");
    let fixture = Arc::new(fixture(&context).await);
    let repository = LaunchRepository::new(context.pool.clone());
    let pool = context.pool.clone();

    for round in 0..20 {
        let label = format!("race-cancel-{round}");
        let (holder, _) = start(&repository, &fixture, &label, LabelConcurrency::Queue).await;
        let (waiter, outcome) = start(&repository, &fixture, &label, LabelConcurrency::Queue).await;
        assert!(matches!(outcome, InitialLaunchOutcome::Waiting(_)));

        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let ending = {
            let (pool, barrier, holder) = (pool.clone(), barrier.clone(), holder.clone());
            tokio::spawn(async move {
                barrier.wait().await;
                set_status(&pool, &holder, "completed").await;
            })
        };
        let cancelling = {
            let (repository, barrier, waiter) =
                (repository.clone(), barrier.clone(), waiter.clone());
            tokio::spawn(async move {
                barrier.wait().await;
                repository
                    .cancel_waiting(&waiter)
                    .await
                    .expect("cancel_waiting must not deadlock")
            })
        };
        ending.await.unwrap();
        let cancelled = cancelling.await.unwrap();
        let state = launch_state(&pool, &waiter).await;
        if cancelled.is_some() {
            assert_eq!(state, "cancelled", "round {round}");
        } else {
            assert_eq!(state, "queued", "round {round}");
        }
    }

    context.cleanup().await;
}
