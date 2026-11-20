// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Which version of a workflow is current when none was chosen
//! (docs/workflow-authoring-gaps.md, 1).
//!
//! A save that does not compile does not take a workflow over: the current
//! version is the one chosen, else the latest that is not known to fail to
//! compile (one that compiled, or one not compiled yet), else the latest.
//!
//! Requires the explicit `db-integration-tests` feature and a live Postgres.

use runtara_server::api::repositories::workflows::WorkflowRepository;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

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

/// A workflow with `versions` versions, each compiled as `statuses` says
/// (`None`: never compiled). Returns (tenant, workflow id).
async fn seed(pool: &PgPool, statuses: &[Option<&str>]) -> (String, String) {
    let tenant = format!("t-{}", Uuid::new_v4());
    let workflow_id = Uuid::new_v4().to_string();
    let latest = statuses.len() as i32;
    sqlx::query(
        "INSERT INTO workflows (tenant_id, workflow_id, version_count, latest_version)
         VALUES ($1, $2, $3, $3)",
    )
    .bind(&tenant)
    .bind(&workflow_id)
    .bind(latest)
    .execute(pool)
    .await
    .expect("seeding a workflow must succeed");
    for (index, status) in statuses.iter().enumerate() {
        let version = index as i32 + 1;
        let definition = json!({"name": format!("v{version}"), "steps": {}, "executionPlan": []});
        let file_size = serde_json::to_vec(&definition).unwrap().len() as i32;
        sqlx::query(
            "INSERT INTO workflow_definitions (tenant_id, workflow_id, version, definition, file_size, track_events)
             VALUES ($1, $2, $3, $4, $5, false)",
        )
        .bind(&tenant)
        .bind(&workflow_id)
        .bind(version)
        .bind(&definition)
        .bind(file_size)
        .execute(pool)
        .await
        .expect("seeding a version must succeed");
        if let Some(status) = status {
            sqlx::query(
                "INSERT INTO workflow_compilations (tenant_id, workflow_id, version, compilation_status, translated_path)
                 VALUES ($1, $2, $3, $4, '/tmp/fixture')",
            )
            .bind(&tenant)
            .bind(&workflow_id)
            .bind(version)
            .bind(status)
            .execute(pool)
            .await
            .expect("seeding a compilation must succeed");
        }
    }
    (tenant, workflow_id)
}

async fn current(pool: &PgPool, tenant: &str, workflow_id: &str) -> (Option<i32>, Option<i32>) {
    let repository = WorkflowRepository::new(pool.clone());
    (
        repository
            .get_current_or_latest_version(tenant, workflow_id)
            .await
            .unwrap(),
        repository
            .get_latest_runnable_version(tenant, workflow_id)
            .await
            .unwrap(),
    )
}

#[tokio::test]
async fn a_version_that_failed_to_compile_is_not_current() {
    let pool = get_test_pool().await;
    let (tenant, workflow) = seed(
        &pool,
        &[
            Some("success"),
            Some("success"),
            Some("failed"),
            Some("failed"),
        ],
    )
    .await;
    assert_eq!(current(&pool, &tenant, &workflow).await, (Some(2), Some(2)));
}

#[tokio::test]
async fn a_version_not_compiled_yet_is_current() {
    let pool = get_test_pool().await;
    // Compiled on demand, as before: save then run runs what was saved.
    let (tenant, workflow) = seed(&pool, &[Some("success"), Some("failed"), None]).await;
    assert_eq!(current(&pool, &tenant, &workflow).await, (Some(3), Some(3)));
    let (tenant, workflow) = seed(&pool, &[Some("success"), Some("in_progress")]).await;
    assert_eq!(current(&pool, &tenant, &workflow).await, (Some(2), Some(2)));
}

#[tokio::test]
async fn a_chosen_version_stays_current() {
    let pool = get_test_pool().await;
    let (tenant, workflow) = seed(&pool, &[Some("success"), Some("success"), None]).await;
    sqlx::query(
        "UPDATE workflows SET current_version = 1 WHERE tenant_id = $1 AND workflow_id = $2",
    )
    .bind(&tenant)
    .bind(&workflow)
    .execute(&pool)
    .await
    .unwrap();
    // What the API shows keeps the choice; the engine's own choice is its
    // record, so the latest runnable is what it runs when it has none.
    assert_eq!(current(&pool, &tenant, &workflow).await, (Some(1), Some(3)));
}

#[tokio::test]
async fn when_every_version_failed_the_latest_is_current() {
    let pool = get_test_pool().await;
    let (tenant, workflow) = seed(&pool, &[Some("failed"), Some("failed")]).await;
    assert_eq!(current(&pool, &tenant, &workflow).await, (Some(2), Some(2)));
}

#[tokio::test]
async fn the_version_list_marks_the_same_version_active() {
    let pool = get_test_pool().await;
    let (tenant, workflow) = seed(&pool, &[Some("success"), Some("failed")]).await;
    let versions = WorkflowRepository::new(pool.clone())
        .list_versions(&tenant, &workflow)
        .await
        .unwrap();
    let active: Vec<i32> = versions
        .iter()
        .filter(|version| version.is_active)
        .map(|version| version.version_number)
        .collect();
    assert_eq!(active, vec![1]);
}
