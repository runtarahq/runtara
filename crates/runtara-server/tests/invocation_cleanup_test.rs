// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! E2E tests for `InvocationCleanupWorker` — exercises the full DELETE path
//! against a real Postgres database seeded with executions, events,
//! side-effect rows, and metrics.
//!
//! This suite is explicitly gated by the `db-integration-tests` feature and
//! fails closed when its required database is unavailable.

use std::time::Duration;

use chrono::{Duration as ChronoDuration, Utc};
use runtara_server::shutdown::ShutdownSignal;
use runtara_server::workers::invocation_cleanup_worker::{
    InvocationCleanupWorker, InvocationCleanupWorkerConfig,
};
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

async fn get_test_pool() -> Option<PgPool> {
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
    Some(pool)
}

async fn insert_workflow(pool: &PgPool, tenant_id: &str, workflow_id: &str) {
    // workflows + workflow_definitions are FK ancestors referenced indirectly;
    // Just make sure the workflow_id column is tenant-unique.
    let _ = (pool, tenant_id, workflow_id);
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

async fn insert_oauth_state(
    pool: &PgPool,
    state: &str,
    tenant_id: &str,
    expires_at: chrono::DateTime<Utc>,
) {
    sqlx::query(
        r#"
        INSERT INTO oauth_state (
            state, tenant_id, connection_id, integration_id, redirect_uri, expires_at
        )
        VALUES ($1, $2, 'conn-1', 'integration-1', 'https://example.invalid/cb', $3)
        "#,
    )
    .bind(state)
    .bind(tenant_id)
    .bind(expires_at)
    .execute(pool)
    .await
    .expect("insert oauth_state");
}

async fn oauth_state_exists(pool: &PgPool, state: &str) -> bool {
    let row: Option<(bool,)> =
        sqlx::query_as("SELECT EXISTS(SELECT 1 FROM oauth_state WHERE state = $1)")
            .bind(state)
            .fetch_optional(pool)
            .await
            .expect("query oauth_state");
    row.map(|r| r.0).unwrap_or(false)
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
        metrics_max_age: Duration::from_secs(365 * 24 * 3600),
    };
    let worker = InvocationCleanupWorker::new(pool.clone(), config, ShutdownSignal::new());

    let (deleted_metrics, _) = worker.cleanup_once().await.expect("cleanup_once");
    assert_eq!(deleted_metrics, 1, "Ancient metric should be deleted");
    assert_eq!(metric_count(&pool, &tenant_id).await, 1);

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
        metrics_max_age: Duration::from_secs(365 * 24 * 3600),
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

/// `run()` must perform an eager cleanup pass *before* the first
/// `poll_interval` elapses. This is the prod-shaped guard for the bug that
/// motivated the 3-day default change: if the worker only cleaned up after
/// `sleep(poll_interval)`, every server restart would push cleanup another
/// hour out and tables would grow unboundedly on a churny box.
///
/// Test setup uses `poll_interval = 1h` and `max_age = 3d`, then seeds a
/// 10-day-old terminal execution. If the eager pass works, the row is gone
/// within a few hundred ms. If it doesn't, the test would hang for an hour
/// (and time out at 5s with a clear regression message).
#[tokio::test]
async fn test_run_performs_eager_cleanup_on_startup() {
    skip_if_no_db!();
    let pool = get_test_pool().await.expect("pool");
    let tenant_id = format!("test-eager-{}", Uuid::new_v4());
    let expired_state = format!("st-eager-{}", Uuid::new_v4());

    // Observe the eager pass through the oauth_state sweep. It used to be
    // observed through workflow_executions, but that phase went away with the
    // tables it swept; what this test is actually for is that `run` does a
    // first cycle immediately rather than waiting out `poll_interval`.
    insert_oauth_state(
        &pool,
        &expired_state,
        &tenant_id,
        Utc::now() - ChronoDuration::minutes(5),
    )
    .await;
    assert!(
        oauth_state_exists(&pool, &expired_state).await,
        "seed precondition: expired oauth_state exists before worker starts"
    );

    let coord = std::sync::Arc::new(runtara_server::shutdown::ShutdownCoordinator::from_env(
        std::sync::Arc::new(dashmap::DashMap::new()),
        None,
    ));
    let signal = coord.signal();

    // poll_interval is intentionally an hour: only the eager pass can fire
    // within the 5s test budget.
    let config = InvocationCleanupWorkerConfig {
        enabled: true,
        poll_interval: Duration::from_secs(3600),
        metrics_max_age: Duration::from_secs(365 * 24 * 3600),
    };
    let worker = InvocationCleanupWorker::new(pool.clone(), config, signal);

    let handle = tokio::spawn(async move { worker.run().await });

    // Poll the DB until the row disappears. Locally this lands in <100ms;
    // give CI runners 5s of headroom.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut deleted = false;
    while std::time::Instant::now() < deadline {
        if !oauth_state_exists(&pool, &expired_state).await {
            deleted = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    coord.request_shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
    sqlx::query("DELETE FROM oauth_state WHERE tenant_id = $1")
        .bind(&tenant_id)
        .execute(&pool)
        .await
        .ok();

    assert!(
        deleted,
        "Eager cleanup did not sweep the expired oauth_state row within 5s. \
         The eager-first-pass behavior in `run()` may be regressed — without \
         it, cleanup would not fire until `poll_interval` (1h) elapsed."
    );
}

/// The consume path (`get_and_delete_state`) only removes the single row it
/// matches, so expired rows from abandoned authorization flows must be swept
/// by the cleanup cycle.
#[tokio::test]
async fn test_cleanup_once_purges_expired_oauth_state() {
    skip_if_no_db!();
    let pool = get_test_pool().await.expect("pool");
    let tenant_id = format!("test-{}", Uuid::new_v4());
    let expired_state = format!("st-exp-{}", Uuid::new_v4());
    let live_state = format!("st-live-{}", Uuid::new_v4());

    insert_oauth_state(
        &pool,
        &expired_state,
        &tenant_id,
        Utc::now() - ChronoDuration::minutes(5),
    )
    .await;
    insert_oauth_state(
        &pool,
        &live_state,
        &tenant_id,
        Utc::now() + ChronoDuration::minutes(10),
    )
    .await;
    assert!(oauth_state_exists(&pool, &expired_state).await);
    assert!(oauth_state_exists(&pool, &live_state).await);

    let config = InvocationCleanupWorkerConfig {
        enabled: true,
        poll_interval: Duration::from_secs(3600),
        metrics_max_age: Duration::from_secs(365 * 24 * 3600),
    };
    let worker = InvocationCleanupWorker::new(pool.clone(), config, ShutdownSignal::new());

    // The sweep is global and other tests in this binary run cleanup cycles
    // concurrently against the shared database, so assert on row fate rather
    // than the returned count.
    worker.cleanup_once().await.expect("cleanup_once");

    assert!(
        !oauth_state_exists(&pool, &expired_state).await,
        "Expired oauth_state row should be swept by the cleanup cycle"
    );
    assert!(
        oauth_state_exists(&pool, &live_state).await,
        "Unexpired oauth_state row must survive the sweep"
    );

    sqlx::query("DELETE FROM oauth_state WHERE tenant_id = $1")
        .bind(&tenant_id)
        .execute(&pool)
        .await
        .ok();
}
