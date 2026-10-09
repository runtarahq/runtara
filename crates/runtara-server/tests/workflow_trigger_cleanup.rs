// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Deleting a workflow must stop its invocation triggers from firing.
//!
//! `invocation_trigger` has no foreign key to `workflows`, and the cron
//! scheduler selects triggers on `active = true` alone, so these tests pin
//! both halves of the cleanup through the production repository SQL:
//! `WorkflowRepository::delete_workflow` deactivates the deleted workflow's
//! triggers, and `TriggerRepository::deactivate_orphaned` (run by the cron
//! scheduler every poll) switches off triggers whose workflow is already gone.
//! Both apply one orphan rule: workflow ids are only unique per tenant, so a
//! tenant's trigger needs a live workflow in its own tenant, while a global
//! trigger needs one in any tenant. `newest_live_channel_trigger`, which picks
//! the Channel trigger a shared webhook must stay registered for, follows the
//! same rule.
//!
//! Requires the explicit `db-integration-tests` feature and a live Postgres.

use runtara_server::api::dto::triggers::{CreateInvocationTriggerRequest, TriggerType};
use runtara_server::api::repositories::triggers::TriggerRepository;
use runtara_server::api::repositories::workflows::WorkflowRepository;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

macro_rules! skip_if_no_db {
    () => {
        assert!(
            std::env::var("TEST_RUNTARA_SERVER_DATABASE_URL").is_ok()
                || std::env::var("RUNTARA_SERVER_DATABASE_URL").is_ok(),
            "db-integration-tests requires TEST_RUNTARA_SERVER_DATABASE_URL or RUNTARA_SERVER_DATABASE_URL"
        );
    };
}

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

async fn get_test_pool() -> PgPool {
    let url = std::env::var("TEST_RUNTARA_SERVER_DATABASE_URL")
        .or_else(|_| std::env::var("RUNTARA_SERVER_DATABASE_URL"))
        .expect("db-integration-tests requires a server database URL");
    let pool = PgPool::connect(&url)
        .await
        .expect("required server test database must accept connections");
    MIGRATOR
        .run(&pool)
        .await
        .expect("required server migrations must succeed");
    pool
}

async fn seed_workflow(repo: &WorkflowRepository, tenant: &str) -> String {
    let workflow_id = Uuid::new_v4().to_string();
    seed_workflow_with_id(repo, tenant, &workflow_id).await;
    workflow_id
}

/// Seed a workflow row under a caller-chosen id, so two tenants can share one.
async fn seed_workflow_with_id(repo: &WorkflowRepository, tenant: &str, workflow_id: &str) {
    repo.create(tenant, workflow_id, None, &format!("wf-{workflow_id}"), "/")
        .await
        .expect("seed workflow row");
}

/// Soft-delete a workflow behind the repository's back, leaving its triggers
/// active: the state left behind by deletes that predate trigger deactivation.
async fn soft_delete_behind_repository(pool: &PgPool, tenant: &str, workflow_id: &str) {
    sqlx::query(
        "UPDATE workflows SET deleted_at = NOW() WHERE tenant_id = $1 AND workflow_id = $2",
    )
    .bind(tenant)
    .bind(workflow_id)
    .execute(pool)
    .await
    .expect("soft-delete workflow");
}

/// Seed an active CRON trigger; `tenant = None` makes it a global trigger.
async fn seed_trigger(pool: &PgPool, tenant: Option<&str>, workflow_id: &str) -> String {
    let request = CreateInvocationTriggerRequest {
        workflow_id: workflow_id.to_string(),
        trigger_type: TriggerType::Cron,
        active: true,
        configuration: Some(json!({ "expression": "* * * * *" })),
        remote_tenant_id: None,
        single_instance: false,
    };
    TriggerRepository::new(pool.clone())
        .create(&request, tenant, None)
        .await
        .expect("seed trigger row")
        .id
}

/// Seed an active Channel trigger on `connection_id`, created `minutes_ago`
/// so tests control which trigger is newest.
async fn seed_channel_trigger(
    pool: &PgPool,
    tenant: &str,
    workflow_id: &str,
    connection_id: &str,
    minutes_ago: i32,
) -> String {
    let request = CreateInvocationTriggerRequest {
        workflow_id: workflow_id.to_string(),
        trigger_type: TriggerType::Channel,
        active: true,
        configuration: Some(json!({ "connection_id": connection_id })),
        remote_tenant_id: None,
        single_instance: false,
    };
    let id = TriggerRepository::new(pool.clone())
        .create(&request, Some(tenant), None)
        .await
        .expect("seed channel trigger row")
        .id;
    sqlx::query(
        "UPDATE invocation_trigger SET created_at = NOW() - make_interval(mins => $2) WHERE id = $1",
    )
    .bind(&id)
    .bind(minutes_ago)
    .execute(pool)
    .await
    .expect("backdate channel trigger");
    id
}

async fn is_active(pool: &PgPool, trigger_id: &str) -> bool {
    sqlx::query_scalar("SELECT active FROM invocation_trigger WHERE id = $1")
        .bind(trigger_id)
        .fetch_one(pool)
        .await
        .expect("trigger row must exist")
}

async fn newest_live_channel_trigger_id(
    triggers: &TriggerRepository,
    connection_id: &str,
    tenant: &str,
) -> Option<String> {
    triggers
        .newest_live_channel_trigger(connection_id, tenant, None)
        .await
        .expect("newest live channel trigger lookup")
        .map(|t| t.id)
}

async fn cleanup(pool: &PgPool, tenant: &str, workflow_ids: &[&str]) {
    for workflow_id in workflow_ids {
        let _ = sqlx::query("DELETE FROM invocation_trigger WHERE workflow_id = $1")
            .bind(workflow_id)
            .execute(pool)
            .await;
    }
    let _ = sqlx::query("DELETE FROM workflows WHERE tenant_id = $1")
        .bind(tenant)
        .execute(pool)
        .await;
}

#[tokio::test]
async fn delete_workflow_deactivates_its_triggers_only() {
    skip_if_no_db!();
    let pool = get_test_pool().await;
    let repo = WorkflowRepository::new(pool.clone());
    let tenant = format!("t-{}", Uuid::new_v4());

    let deleted_wf = seed_workflow(&repo, &tenant).await;
    let kept_wf = seed_workflow(&repo, &tenant).await;
    let tenant_trigger = seed_trigger(&pool, Some(&tenant), &deleted_wf).await;
    let global_trigger = seed_trigger(&pool, None, &deleted_wf).await;
    let kept_trigger = seed_trigger(&pool, Some(&tenant), &kept_wf).await;

    let outcome = repo
        .delete_workflow(&tenant, &deleted_wf)
        .await
        .expect("delete workflow");

    let mut returned: Vec<&str> = outcome
        .deactivated_triggers
        .iter()
        .map(|t| t.id.as_str())
        .collect();
    returned.sort_unstable();
    let mut expected = vec![tenant_trigger.as_str(), global_trigger.as_str()];
    expected.sort_unstable();
    assert_eq!(returned, expected, "both of the workflow's triggers");
    assert!(
        outcome.deactivated_triggers.iter().all(|t| !t.active),
        "returned rows reflect the update"
    );

    assert!(!is_active(&pool, &tenant_trigger).await);
    assert!(!is_active(&pool, &global_trigger).await);
    assert!(
        is_active(&pool, &kept_trigger).await,
        "another workflow's trigger is untouched"
    );

    // A repeated delete finds nothing left to deactivate.
    let again = repo
        .delete_workflow(&tenant, &deleted_wf)
        .await
        .expect("repeat delete");
    assert!(again.deactivated_triggers.is_empty());

    cleanup(&pool, &tenant, &[&deleted_wf, &kept_wf]).await;
}

#[tokio::test]
async fn deactivate_orphaned_switches_off_only_dead_workflow_triggers() {
    skip_if_no_db!();
    let pool = get_test_pool().await;
    let repo = WorkflowRepository::new(pool.clone());
    let triggers = TriggerRepository::new(pool.clone());
    let tenant = format!("t-{}", Uuid::new_v4());

    let soft_deleted_wf = seed_workflow(&repo, &tenant).await;
    soft_delete_behind_repository(&pool, &tenant, &soft_deleted_wf).await;
    let soft_deleted_trigger = seed_trigger(&pool, Some(&tenant), &soft_deleted_wf).await;

    // A trigger pointing at a workflow id that never existed.
    let missing_wf = Uuid::new_v4().to_string();
    let missing_trigger = seed_trigger(&pool, Some(&tenant), &missing_wf).await;

    let live_wf = seed_workflow(&repo, &tenant).await;
    let live_trigger = seed_trigger(&pool, Some(&tenant), &live_wf).await;

    // The database is shared, so other runs' orphans may be swept up too:
    // assert membership rather than the exact set.
    let deactivated = triggers
        .deactivate_orphaned(&tenant)
        .await
        .expect("deactivate orphaned");
    let ids: Vec<&str> = deactivated.iter().map(|t| t.id.as_str()).collect();
    assert!(ids.contains(&soft_deleted_trigger.as_str()));
    assert!(ids.contains(&missing_trigger.as_str()));
    assert!(!ids.contains(&live_trigger.as_str()));

    assert!(!is_active(&pool, &soft_deleted_trigger).await);
    assert!(!is_active(&pool, &missing_trigger).await);
    assert!(
        is_active(&pool, &live_trigger).await,
        "a live workflow's trigger is untouched"
    );

    // Idempotent: nothing of this tenant's is left to deactivate.
    let again = triggers
        .deactivate_orphaned(&tenant)
        .await
        .expect("repeat deactivate orphaned");
    assert!(
        again
            .iter()
            .all(|t| t.tenant_id.as_deref() != Some(tenant.as_str())),
        "no trigger of this tenant is deactivated twice"
    );

    cleanup(&pool, &tenant, &[&soft_deleted_wf, &missing_wf, &live_wf]).await;
}

#[tokio::test]
async fn delete_workflow_keeps_global_trigger_while_another_tenant_has_the_workflow() {
    skip_if_no_db!();
    let pool = get_test_pool().await;
    let repo = WorkflowRepository::new(pool.clone());
    let tenant_a = format!("t-{}", Uuid::new_v4());
    let tenant_b = format!("t-{}", Uuid::new_v4());

    // Workflow ids are only unique per tenant: both tenants hold this one.
    let shared_wf = Uuid::new_v4().to_string();
    seed_workflow_with_id(&repo, &tenant_a, &shared_wf).await;
    seed_workflow_with_id(&repo, &tenant_b, &shared_wf).await;
    let a_trigger = seed_trigger(&pool, Some(&tenant_a), &shared_wf).await;
    let b_trigger = seed_trigger(&pool, Some(&tenant_b), &shared_wf).await;
    let global_trigger = seed_trigger(&pool, None, &shared_wf).await;

    let outcome = repo
        .delete_workflow(&tenant_a, &shared_wf)
        .await
        .expect("delete tenant A's workflow");
    let ids: Vec<&str> = outcome
        .deactivated_triggers
        .iter()
        .map(|t| t.id.as_str())
        .collect();
    assert_eq!(ids, vec![a_trigger.as_str()], "only A's own trigger");
    assert!(!is_active(&pool, &a_trigger).await);
    assert!(
        is_active(&pool, &global_trigger).await,
        "B still has a live workflow with this id"
    );
    assert!(
        is_active(&pool, &b_trigger).await,
        "another tenant's trigger is untouched"
    );

    // Once the last live copy goes, the global trigger goes with it.
    let outcome = repo
        .delete_workflow(&tenant_b, &shared_wf)
        .await
        .expect("delete tenant B's workflow");
    let mut ids: Vec<&str> = outcome
        .deactivated_triggers
        .iter()
        .map(|t| t.id.as_str())
        .collect();
    ids.sort_unstable();
    let mut expected = vec![b_trigger.as_str(), global_trigger.as_str()];
    expected.sort_unstable();
    assert_eq!(ids, expected);
    assert!(!is_active(&pool, &global_trigger).await);

    cleanup(&pool, &tenant_a, &[&shared_wf]).await;
    cleanup(&pool, &tenant_b, &[&shared_wf]).await;
}

#[tokio::test]
async fn deactivate_orphaned_requires_a_live_workflow_in_the_triggers_own_tenant() {
    skip_if_no_db!();
    let pool = get_test_pool().await;
    let repo = WorkflowRepository::new(pool.clone());
    let triggers = TriggerRepository::new(pool.clone());
    let tenant_a = format!("t-{}", Uuid::new_v4());
    let tenant_b = format!("t-{}", Uuid::new_v4());

    // A's copy is dead, B's copy of the same id is live.
    let shared_wf = Uuid::new_v4().to_string();
    seed_workflow_with_id(&repo, &tenant_a, &shared_wf).await;
    seed_workflow_with_id(&repo, &tenant_b, &shared_wf).await;
    soft_delete_behind_repository(&pool, &tenant_a, &shared_wf).await;
    let a_trigger = seed_trigger(&pool, Some(&tenant_a), &shared_wf).await;
    let global_trigger = seed_trigger(&pool, None, &shared_wf).await;

    let deactivated = triggers
        .deactivate_orphaned(&tenant_a)
        .await
        .expect("deactivate orphaned");
    let ids: Vec<&str> = deactivated.iter().map(|t| t.id.as_str()).collect();
    assert!(
        ids.contains(&a_trigger.as_str()),
        "B's live copy must not keep A's orphan alive"
    );
    assert!(
        !ids.contains(&global_trigger.as_str()),
        "a global trigger stays while any tenant has the workflow"
    );
    assert!(!is_active(&pool, &a_trigger).await);
    assert!(is_active(&pool, &global_trigger).await);

    cleanup(&pool, &tenant_a, &[&shared_wf]).await;
    cleanup(&pool, &tenant_b, &[&shared_wf]).await;
}

#[tokio::test]
async fn newest_live_channel_trigger_skips_inactive_and_dead_workflow_triggers() {
    skip_if_no_db!();
    let pool = get_test_pool().await;
    let repo = WorkflowRepository::new(pool.clone());
    let triggers = TriggerRepository::new(pool.clone());
    let tenant = format!("t-{}", Uuid::new_v4());
    let connection = format!("conn-{}", Uuid::new_v4());

    let older_wf = seed_workflow(&repo, &tenant).await;
    let newer_wf = seed_workflow(&repo, &tenant).await;
    let dead_wf = seed_workflow(&repo, &tenant).await;
    soft_delete_behind_repository(&pool, &tenant, &dead_wf).await;
    let older = seed_channel_trigger(&pool, &tenant, &older_wf, &connection, 30).await;
    let newer = seed_channel_trigger(&pool, &tenant, &newer_wf, &connection, 20).await;
    // Newest of all, but its workflow is gone: it is about to be swept and
    // must not decide which secret the webhook keeps.
    let _dead = seed_channel_trigger(&pool, &tenant, &dead_wf, &connection, 10).await;

    assert_eq!(
        newest_live_channel_trigger_id(&triggers, &connection, &tenant).await,
        Some(newer.clone())
    );
    assert_eq!(
        triggers
            .newest_live_channel_trigger(&connection, &tenant, Some(&newer))
            .await
            .expect("newest live channel trigger lookup")
            .map(|t| t.id),
        Some(older.clone()),
        "an excepted trigger is skipped"
    );

    sqlx::query("UPDATE invocation_trigger SET active = false WHERE id = $1")
        .bind(&newer)
        .execute(&pool)
        .await
        .expect("deactivate newer trigger");
    assert_eq!(
        newest_live_channel_trigger_id(&triggers, &connection, &tenant).await,
        Some(older.clone()),
        "an inactive trigger is skipped"
    );

    soft_delete_behind_repository(&pool, &tenant, &older_wf).await;
    assert_eq!(
        newest_live_channel_trigger_id(&triggers, &connection, &tenant).await,
        None,
        "no live trigger is left on the connection"
    );

    cleanup(&pool, &tenant, &[&older_wf, &newer_wf, &dead_wf]).await;
}
