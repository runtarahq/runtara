// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! E2E tests for `InvocationCleanupWorker` — exercises the full DELETE path
//! against a real Postgres database seeded with executions, events,
//! side-effect rows, and metrics.
//!
//! Skips if neither `TEST_RUNTARA_SERVER_DATABASE_URL` nor
//! `RUNTARA_SERVER_DATABASE_URL` is set. Mirrors the skip-behavior in
//! `runtara-environment/tests/db_cleanup_worker_test.rs`.

use std::time::Duration;

use chrono::{Duration as ChronoDuration, Utc};
use runtara_server::shutdown::ShutdownSignal;
use runtara_server::workers::invocation_cleanup_worker::{
    InvocationCleanupWorker, InvocationCleanupWorkerConfig,
};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

macro_rules! skip_if_no_db {
    () => {
        if std::env::var("TEST_RUNTARA_SERVER_DATABASE_URL").is_err()
            && std::env::var("RUNTARA_SERVER_DATABASE_URL").is_err()
        {
            eprintln!(
                "Skipping test: TEST_RUNTARA_SERVER_DATABASE_URL or RUNTARA_SERVER_DATABASE_URL not set"
            );
            return;
        }
    };
}

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

async fn get_test_pool() -> Option<PgPool> {
    let url = std::env::var("TEST_RUNTARA_SERVER_DATABASE_URL")
        .or_else(|_| std::env::var("RUNTARA_SERVER_DATABASE_URL"))
        .ok()?;
    let pool = PgPool::connect(&url).await.ok()?;
    MIGRATOR.run(&pool).await.ok()?;
    Some(pool)
}

async fn insert_workflow(pool: &PgPool, tenant_id: &str, workflow_id: &str) {
    // workflows + workflow_definitions are FK ancestors referenced indirectly;
    // workflow_executions does not FK them, so we can skip.
    // Just make sure the workflow_id column is tenant-unique.
    let _ = (pool, tenant_id, workflow_id);
}

async fn insert_execution(
    pool: &PgPool,
    instance_id: Uuid,
    tenant_id: &str,
    workflow_id: &str,
    status: &str,
    created_at: chrono::DateTime<Utc>,
    completed_at: Option<chrono::DateTime<Utc>>,
) {
    sqlx::query(
        r#"
        INSERT INTO workflow_executions (
            instance_id, tenant_id, workflow_id, version, status,
            inputs, created_at, completed_at
        )
        VALUES ($1, $2, $3, 1, $4, $5, $6, $7)
        "#,
    )
    .bind(instance_id)
    .bind(tenant_id)
    .bind(workflow_id)
    .bind(status)
    .bind(json!({}))
    .bind(created_at)
    .bind(completed_at)
    .execute(pool)
    .await
    .expect("insert execution");
}

async fn insert_event(pool: &PgPool, instance_id: Uuid, event_type: &str) {
    sqlx::query(
        r#"
        INSERT INTO workflow_execution_events (instance_id, event_type, event_data)
        VALUES ($1, $2, $3)
        "#,
    )
    .bind(instance_id)
    .bind(event_type)
    .bind(json!({}))
    .execute(pool)
    .await
    .expect("insert event");
}

async fn insert_side_effect(
    pool: &PgPool,
    instance_id: Uuid,
    tenant_id: &str,
    workflow_id: &str,
    op: &str,
) {
    sqlx::query(
        r#"
        INSERT INTO side_effect_usage (
            instance_id, tenant_id, workflow_id, version, operation_type, operation_count
        )
        VALUES ($1, $2, $3, 1, $4, 1)
        ON CONFLICT (instance_id, operation_type) DO NOTHING
        "#,
    )
    .bind(instance_id)
    .bind(tenant_id)
    .bind(workflow_id)
    .bind(op)
    .execute(pool)
    .await
    .expect("insert side_effect");
}

async fn insert_metric(
    pool: &PgPool,
    tenant_id: &str,
    workflow_id: &str,
    hour_bucket: chrono::DateTime<Utc>,
) {
    sqlx::query(
        r#"
        INSERT INTO workflow_metrics_hourly (
            tenant_id, workflow_id, version, hour_bucket, invocation_count
        )
        VALUES ($1, $2, 1, $3, 1)
        ON CONFLICT (tenant_id, workflow_id, version, hour_bucket) DO NOTHING
        "#,
    )
    .bind(tenant_id)
    .bind(workflow_id)
    .bind(hour_bucket)
    .execute(pool)
    .await
    .expect("insert metric");
}

async fn execution_exists(pool: &PgPool, instance_id: Uuid) -> bool {
    let row: Option<(bool,)> =
        sqlx::query_as("SELECT EXISTS(SELECT 1 FROM workflow_executions WHERE instance_id = $1)")
            .bind(instance_id)
            .fetch_optional(pool)
            .await
            .expect("query exec");
    row.map(|r| r.0).unwrap_or(false)
}

async fn event_count(pool: &PgPool, instance_id: Uuid) -> i64 {
    let row: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM workflow_execution_events WHERE instance_id = $1")
            .bind(instance_id)
            .fetch_one(pool)
            .await
            .expect("query events");
    row.0
}

async fn side_effect_count(pool: &PgPool, instance_id: Uuid) -> i64 {
    let row: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM side_effect_usage WHERE instance_id = $1")
            .bind(instance_id)
            .fetch_one(pool)
            .await
            .expect("query side_effect");
    row.0
}

async fn metric_count(pool: &PgPool, tenant_id: &str) -> i64 {
    let row: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM workflow_metrics_hourly WHERE tenant_id = $1")
            .bind(tenant_id)
            .fetch_one(pool)
            .await
            .expect("query metrics");
    row.0
}

async fn cleanup_tenant(pool: &PgPool, tenant_id: &str) {
    sqlx::query("DELETE FROM workflow_metrics_hourly WHERE tenant_id = $1")
        .bind(tenant_id)
        .execute(pool)
        .await
        .ok();
    sqlx::query("DELETE FROM workflow_executions WHERE tenant_id = $1")
        .bind(tenant_id)
        .execute(pool)
        .await
        .ok();
}

#[tokio::test]
async fn test_cleanup_once_deletes_terminal_and_preserves_running() {
    skip_if_no_db!();
    let pool = get_test_pool().await.expect("pool");
    let tenant_id = format!("test-{}", Uuid::new_v4());
    let workflow_id = format!("wf-{}", Uuid::new_v4());
    insert_workflow(&pool, &tenant_id, &workflow_id).await;

    let old = Utc::now() - ChronoDuration::days(10);
    let recent = Utc::now() - ChronoDuration::hours(1);

    let old_completed = Uuid::new_v4();
    let old_failed = Uuid::new_v4();
    let old_timeout = Uuid::new_v4();
    let old_cancelled = Uuid::new_v4();
    let old_running = Uuid::new_v4();
    let old_queued = Uuid::new_v4();
    let recent_completed = Uuid::new_v4();

    insert_execution(
        &pool,
        old_completed,
        &tenant_id,
        &workflow_id,
        "completed",
        old,
        Some(old),
    )
    .await;
    insert_execution(
        &pool,
        old_failed,
        &tenant_id,
        &workflow_id,
        "failed",
        old,
        Some(old),
    )
    .await;
    insert_execution(
        &pool,
        old_timeout,
        &tenant_id,
        &workflow_id,
        "timeout",
        old,
        Some(old),
    )
    .await;
    insert_execution(
        &pool,
        old_cancelled,
        &tenant_id,
        &workflow_id,
        "cancelled",
        old,
        Some(old),
    )
    .await;
    insert_execution(
        &pool,
        old_running,
        &tenant_id,
        &workflow_id,
        "running",
        old,
        None,
    )
    .await;
    insert_execution(
        &pool,
        old_queued,
        &tenant_id,
        &workflow_id,
        "queued",
        old,
        None,
    )
    .await;
    insert_execution(
        &pool,
        recent_completed,
        &tenant_id,
        &workflow_id,
        "completed",
        recent,
        Some(recent),
    )
    .await;

    // Add events + side_effects to one victim to verify CASCADE
    insert_event(&pool, old_completed, "started").await;
    insert_event(&pool, old_completed, "progress").await;
    insert_side_effect(&pool, old_completed, &tenant_id, &workflow_id, "http_call").await;

    assert_eq!(event_count(&pool, old_completed).await, 2);
    assert_eq!(side_effect_count(&pool, old_completed).await, 1);

    let config = InvocationCleanupWorkerConfig {
        enabled: true,
        poll_interval: Duration::from_secs(3600),
        max_age: Duration::from_secs(7 * 24 * 3600), // 7d; old is 10d so caught, recent is 1h so preserved
        metrics_max_age: Duration::from_secs(365 * 24 * 3600),
        batch_size: 100,
    };
    let worker = InvocationCleanupWorker::new(pool.clone(), config, ShutdownSignal::new());

    let (deleted_exec, _) = worker.cleanup_once().await.expect("cleanup_once");
    assert_eq!(
        deleted_exec, 4,
        "Four terminal rows older than cutoff should be deleted"
    );

    // Terminal old → gone
    assert!(!execution_exists(&pool, old_completed).await);
    assert!(!execution_exists(&pool, old_failed).await);
    assert!(!execution_exists(&pool, old_timeout).await);
    assert!(!execution_exists(&pool, old_cancelled).await);

    // Non-terminal old → preserved
    assert!(execution_exists(&pool, old_running).await);
    assert!(execution_exists(&pool, old_queued).await);

    // Recent terminal → preserved
    assert!(execution_exists(&pool, recent_completed).await);

    // CASCADE verified
    assert_eq!(event_count(&pool, old_completed).await, 0);
    assert_eq!(side_effect_count(&pool, old_completed).await, 0);

    cleanup_tenant(&pool, &tenant_id).await;
}

#[tokio::test]
async fn test_cleanup_once_prunes_old_metrics_respecting_metrics_ttl() {
    skip_if_no_db!();
    let pool = get_test_pool().await.expect("pool");
    let tenant_id = format!("test-{}", Uuid::new_v4());
    let workflow_id = format!("wf-{}", Uuid::new_v4());
    insert_workflow(&pool, &tenant_id, &workflow_id).await;

    // Metrics far in the past (400d) should be cleaned; recent (30d) preserved.
    let ancient = Utc::now() - ChronoDuration::days(400);
    let recent = Utc::now() - ChronoDuration::days(30);
    insert_metric(&pool, &tenant_id, &workflow_id, ancient).await;
    insert_metric(&pool, &tenant_id, &workflow_id, recent).await;
    assert_eq!(metric_count(&pool, &tenant_id).await, 2);

    let config = InvocationCleanupWorkerConfig {
        enabled: true,
        poll_interval: Duration::from_secs(3600),
        max_age: Duration::from_secs(7 * 24 * 3600),
        metrics_max_age: Duration::from_secs(365 * 24 * 3600),
        batch_size: 100,
    };
    let worker = InvocationCleanupWorker::new(pool.clone(), config, ShutdownSignal::new());

    let (_, deleted_metrics) = worker.cleanup_once().await.expect("cleanup_once");
    assert_eq!(deleted_metrics, 1, "Ancient metric should be deleted");
    assert_eq!(metric_count(&pool, &tenant_id).await, 1);

    cleanup_tenant(&pool, &tenant_id).await;
}

#[tokio::test]
async fn test_cleanup_once_respects_batch_size() {
    skip_if_no_db!();
    let pool = get_test_pool().await.expect("pool");
    let tenant_id = format!("test-{}", Uuid::new_v4());
    let workflow_id = format!("wf-{}", Uuid::new_v4());
    insert_workflow(&pool, &tenant_id, &workflow_id).await;

    let old = Utc::now() - ChronoDuration::days(10);

    // Seed more than batch_size
    for _ in 0..12 {
        let id = Uuid::new_v4();
        insert_execution(
            &pool,
            id,
            &tenant_id,
            &workflow_id,
            "completed",
            old,
            Some(old),
        )
        .await;
    }

    let config = InvocationCleanupWorkerConfig {
        enabled: true,
        poll_interval: Duration::from_secs(3600),
        max_age: Duration::from_secs(7 * 24 * 3600),
        metrics_max_age: Duration::from_secs(365 * 24 * 3600),
        batch_size: 5, // forces at least 3 batches
    };
    let worker = InvocationCleanupWorker::new(pool.clone(), config, ShutdownSignal::new());

    let (deleted_exec, _) = worker.cleanup_once().await.expect("cleanup_once");
    assert_eq!(deleted_exec, 12, "All 12 executions deleted via batching");

    let remaining: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM workflow_executions WHERE tenant_id = $1")
            .bind(&tenant_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(remaining.0, 0);

    cleanup_tenant(&pool, &tenant_id).await;
}

#[tokio::test]
async fn test_run_loop_exits_on_coordinator_shutdown() {
    skip_if_no_db!();
    let pool = get_test_pool().await.expect("pool");

    // Use the coordinator to get a signal we can actually flip from a test —
    // `ShutdownSignal::new()` creates its own atomic and exposes no setter.
    let coord = std::sync::Arc::new(runtara_server::shutdown::ShutdownCoordinator::from_env(
        std::sync::Arc::new(dashmap::DashMap::new()),
        None,
    ));
    let signal = coord.signal();

    let config = InvocationCleanupWorkerConfig {
        enabled: true,
        poll_interval: Duration::from_secs(3600), // long — loop is parked in sleep
        max_age: Duration::from_secs(7 * 24 * 3600),
        metrics_max_age: Duration::from_secs(365 * 24 * 3600),
        batch_size: 100,
    };
    let worker = InvocationCleanupWorker::new(pool, config, signal);

    let handle = tokio::spawn(async move { worker.run().await });

    // Brief yield so the worker task enters its select!.
    tokio::time::sleep(Duration::from_millis(100)).await;
    coord.request_shutdown();

    tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .expect("worker exited within 2s of shutdown")
        .expect("task did not panic");
}
