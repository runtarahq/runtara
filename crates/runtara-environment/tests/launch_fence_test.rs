// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! The launch fence: a parented `claim_initial` and
//! `Persistence::publish_external_outcome` for the same child exclude each
//! other, whichever comes first, and under a race exactly one wins.

mod common;

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use common::TestContext;
use runtara_core::persistence::{
    ExternalOutcome, ExternalOutcomeKind, ParentLink, Persistence, PublishOutcome,
};
use runtara_environment::launch_queue::{
    EnqueueRequest, InitialLaunchOutcome, InitialLaunchRequest, LaunchKind, LaunchQueueError,
    LaunchRepository,
};
use runtara_store_postgres::PostgresPersistence;
use uuid::Uuid;

struct Family {
    tenant: String,
    image: String,
    parent: String,
    admitted_at: DateTime<Utc>,
}

async fn family(context: &TestContext) -> Family {
    let tenant = format!("launch-fence-{}", Uuid::new_v4());
    let image = context
        .create_test_image(&tenant, "launch-fence")
        .await
        .to_string();
    let parent = Uuid::new_v4().to_string();
    PostgresPersistence::new(context.pool.clone())
        .register_instance(&parent, &tenant)
        .await
        .expect("parent registers");
    Family {
        tenant,
        image,
        parent,
        admitted_at: DateTime::from_timestamp_millis(Utc::now().timestamp_millis()).unwrap(),
    }
}

fn launch(family: &Family, child: &str) -> InitialLaunchRequest {
    InitialLaunchRequest {
        run_label: None,
        parent: Some(ParentLink {
            parent_instance_id: family.parent.clone(),
            parent_close_policy: "cancel".into(),
            admitted_at: family.admitted_at,
        }),
        launch: EnqueueRequest::immediate(
            Uuid::new_v4().to_string(),
            child,
            &family.tenant,
            &family.image,
            LaunchKind::Start,
            Duration::from_secs(600),
        ),
        input: None,
        env: None,
        timeout_seconds: None,
    }
}

fn outcome(family: &Family, child: &str) -> ExternalOutcome {
    ExternalOutcome {
        instance_id: child.into(),
        tenant_id: family.tenant.clone(),
        parent_instance_id: family.parent.clone(),
        outcome: ExternalOutcomeKind::NotStarted,
        reason: Some("execution_outbox_deadline_exceeded".into()),
        admitted_at: family.admitted_at,
        workflow_id: None,
        workflow_version: None,
        run_label: None,
    }
}

#[tokio::test]
async fn a_published_outcome_refuses_the_launch_and_a_launch_refuses_the_outcome() {
    let context = TestContext::new().await.expect("test database");
    let family = family(&context).await;
    let persistence = PostgresPersistence::new(context.pool.clone());
    let repository = LaunchRepository::new(context.pool.clone());

    // Publish, then launch: refused, and nothing is written.
    let unlaunched = Uuid::new_v4().to_string();
    assert_eq!(
        persistence
            .publish_external_outcome(&outcome(&family, &unlaunched))
            .await
            .unwrap(),
        PublishOutcome::Published
    );
    let refused = repository.claim_initial(launch(&family, &unlaunched)).await;
    assert!(
        matches!(
            refused,
            Err(LaunchQueueError::LaunchFenced { ref instance_id, outcome: "not_started" })
                if instance_id == &unlaunched
        ),
        "got {refused:?}"
    );
    assert!(
        persistence
            .get_instance(&unlaunched)
            .await
            .unwrap()
            .is_none()
    );
    let launches: i64 =
        sqlx::query_scalar("SELECT count(*) FROM instance_launches WHERE instance_id = $1")
            .bind(&unlaunched)
            .fetch_one(&context.pool)
            .await
            .unwrap();
    assert_eq!(launches, 0);

    // Launch, then publish: the launch wins.
    let launched = Uuid::new_v4().to_string();
    assert!(matches!(
        repository
            .claim_initial(launch(&family, &launched))
            .await
            .unwrap(),
        InitialLaunchOutcome::Enqueued(_)
    ));
    assert_eq!(
        persistence
            .publish_external_outcome(&outcome(&family, &launched))
            .await
            .unwrap(),
        PublishOutcome::Launched
    );
    assert!(
        persistence
            .get_external_outcome(&family.tenant, &launched)
            .await
            .unwrap()
            .is_none()
    );
    // A replayed launch of the launched child is still the existing launch.
    assert!(matches!(
        repository
            .claim_initial(launch(&family, &launched))
            .await
            .unwrap(),
        InitialLaunchOutcome::ExistingLaunch(_)
    ));
    context.cleanup_tenant(&family.tenant).await;
}

/// The S0.4 launch-fence race: many children, each published and launched
/// concurrently, in one run. Exactly one side wins each, and no id ends up
/// with both an instance row and an outcome.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn publish_and_launch_race_has_exactly_one_winner() {
    const ITERATIONS: usize = 300;
    let context = TestContext::new().await.expect("test database");
    let family = Arc::new(family(&context).await);
    let persistence = Arc::new(PostgresPersistence::new(context.pool.clone()));
    let repository = Arc::new(LaunchRepository::new(context.pool.clone()));
    let (mut launches_won, mut outcomes_won) = (0usize, 0usize);
    let mut children = Vec::with_capacity(ITERATIONS);

    for i in 0..ITERATIONS {
        let child = Uuid::new_v4().to_string();
        children.push(child.clone());
        let publish = {
            let (persistence, family, child) =
                (Arc::clone(&persistence), Arc::clone(&family), child.clone());
            tokio::spawn(async move {
                persistence
                    .publish_external_outcome(&outcome(&family, &child))
                    .await
            })
        };
        let claim = {
            let (repository, family, child) =
                (Arc::clone(&repository), Arc::clone(&family), child.clone());
            tokio::spawn(async move { repository.claim_initial(launch(&family, &child)).await })
        };
        // Alternate which side is spawned first as well.
        let (published, claimed) = if i % 2 == 0 {
            (publish.await.unwrap(), claim.await.unwrap())
        } else {
            let claimed = claim.await.unwrap();
            (publish.await.unwrap(), claimed)
        };
        match (published.expect("publish"), claimed) {
            (PublishOutcome::Launched, Ok(InitialLaunchOutcome::Enqueued(_))) => launches_won += 1,
            (PublishOutcome::Published, Err(LaunchQueueError::LaunchFenced { .. })) => {
                outcomes_won += 1
            }
            other => panic!("iteration {i}: two winners or none: {other:?}"),
        }
    }
    assert_eq!(launches_won + outcomes_won, ITERATIONS);
    eprintln!("launch fence race: {launches_won} launches, {outcomes_won} outcomes won");

    let both: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM instance_external_outcomes AS o \
         JOIN instances AS i ON i.instance_id = o.instance_id \
         WHERE o.instance_id = ANY($1)",
    )
    .bind(&children)
    .fetch_one(&context.pool)
    .await
    .unwrap();
    assert_eq!(both, 0, "no id has both an instance row and an outcome");
    let (rows, outcomes): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM instances WHERE instance_id = ANY($1)), \
                (SELECT count(*) FROM instance_external_outcomes WHERE instance_id = ANY($1))",
    )
    .bind(&children)
    .fetch_one(&context.pool)
    .await
    .unwrap();
    assert_eq!(rows as usize, launches_won);
    assert_eq!(outcomes as usize, outcomes_won);

    sqlx::query("DELETE FROM instance_external_outcomes WHERE instance_id = ANY($1)")
        .bind(&children)
        .execute(&context.pool)
        .await
        .unwrap();
    context.cleanup_tenant(&family.tenant).await;
}
