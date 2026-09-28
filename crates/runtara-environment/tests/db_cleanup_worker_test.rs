// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Tests for db_cleanup_worker module - cleaning up old database records.

mod common;

use chrono::{Duration as ChronoDuration, Utc};
use runtara_environment::db_cleanup_worker::{DbCleanupWorker, DbCleanupWorkerConfig};
use runtara_store_postgres::PostgresPersistence;
use sqlx::PgPool;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

/// Required preflight for the explicitly feature-gated database suite.
macro_rules! skip_if_no_db {
    () => {
        assert!(
            std::env::var("TEST_ENVIRONMENT_DATABASE_URL").is_ok()
                || std::env::var("RUNTARA_ENVIRONMENT_DATABASE_URL").is_ok(),
            "db-integration-tests requires TEST_ENVIRONMENT_DATABASE_URL or RUNTARA_ENVIRONMENT_DATABASE_URL"
        );
    };
}

/// Serializes every test that sweeps the shared tables.
///
/// The cleanup worker deletes by age across the WHOLE table -- it has no
/// tenant or instance scoping -- so two of these running at once wipe each
/// other's fixtures: a test asserting "my 35-day-old instance is still here"
/// fails because a rival test's worker legitimately swept it. Each test is
/// correct in isolation; it is the shared database that makes them mutually
/// destructive, so they take turns.
///
/// Held across the whole test body, not just the sweep, because the fixtures
/// are created before the worker runs and asserted after it stops.
static SWEEP_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Get a database pool for testing
async fn get_test_pool() -> Option<PgPool> {
    let database_url = std::env::var("TEST_ENVIRONMENT_DATABASE_URL")
        .or_else(|_| std::env::var("RUNTARA_ENVIRONMENT_DATABASE_URL"))
        .expect("db-integration-tests requires an environment database URL");
    let pool = PgPool::connect(&database_url)
        .await
        .expect("required environment test database must accept connections");
    runtara_environment::migrations::run(&pool)
        .await
        .expect("required combined core/environment migrations must succeed");
    Some(pool)
}

/// Create a test image in the database with a unique name
async fn create_test_image(pool: &PgPool, tenant_id: &str) -> String {
    let image_id = Uuid::new_v4().to_string();
    let image_name = format!("test-image-{}", image_id);
    sqlx::query(
        r#"
        INSERT INTO images (image_id, tenant_id, name, description, binary_path)
        VALUES ($1, $2, $3, 'Test image', '/usr/bin/test')
        "#,
    )
    .bind(&image_id)
    .bind(tenant_id)
    .bind(&image_name)
    .execute(pool)
    .await
    .expect("Failed to create test image");
    image_id
}

/// Create a test instance in the database
async fn create_test_instance(
    pool: &PgPool,
    instance_id: &str,
    tenant_id: &str,
    _image_id: &str,
    status: &str,
    finished_at: Option<chrono::DateTime<Utc>>,
) {
    sqlx::query(
        r#"
        INSERT INTO instances (instance_id, tenant_id, status, created_at, started_at, finished_at)
        VALUES ($1, $2, $3::instance_status, NOW() - INTERVAL '2 days', NOW() - INTERVAL '2 days', $4)
        "#,
    )
    .bind(instance_id)
    .bind(tenant_id)
    .bind(status)
    .bind(finished_at)
    .execute(pool)
    .await
    .expect("Failed to create test instance");
}

/// Create a test entry in instance_images table
async fn create_instance_image(pool: &PgPool, instance_id: &str, image_id: &str, tenant_id: &str) {
    sqlx::query(
        r#"
        INSERT INTO instance_images (instance_id, image_id, tenant_id)
        VALUES ($1, $2, $3)
        ON CONFLICT (instance_id) DO NOTHING
        "#,
    )
    .bind(instance_id)
    .bind(image_id)
    .bind(tenant_id)
    .execute(pool)
    .await
    .expect("Failed to create instance_image");
}

/// Create a test entry in container_registry table
async fn create_container_registry(pool: &PgPool, instance_id: &str, tenant_id: &str) {
    sqlx::query(
        r#"
        INSERT INTO container_registry (container_id, launch_id, instance_id, tenant_id, binary_path, started_at)
        VALUES ($1, $1, $2, $3, '/usr/bin/test', NOW())
        ON CONFLICT (instance_id) DO NOTHING
        "#,
    )
    .bind(format!("container-{}", instance_id))
    .bind(instance_id)
    .bind(tenant_id)
    .execute(pool)
    .await
    .expect("Failed to create container_registry entry");
}

/// Check if an instance exists in the database
async fn instance_exists(pool: &PgPool, instance_id: &str) -> bool {
    let result: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM instances WHERE instance_id = $1")
        .bind(instance_id)
        .fetch_optional(pool)
        .await
        .expect("Failed to query instance");
    result.is_some()
}

/// Check if an instance_images entry exists
async fn instance_image_exists(pool: &PgPool, instance_id: &str) -> bool {
    let result: Option<(i32,)> =
        sqlx::query_as("SELECT 1 FROM instance_images WHERE instance_id = $1")
            .bind(instance_id)
            .fetch_optional(pool)
            .await
            .expect("Failed to query instance_images");
    result.is_some()
}

/// Check if a container_registry entry exists
async fn container_registry_exists(pool: &PgPool, instance_id: &str) -> bool {
    let result: Option<(i32,)> =
        sqlx::query_as("SELECT 1 FROM container_registry WHERE instance_id = $1")
            .bind(instance_id)
            .fetch_optional(pool)
            .await
            .expect("Failed to query container_registry");
    result.is_some()
}

/// Cleanup test data
async fn cleanup_test_data(pool: &PgPool, instance_ids: &[&str], image_id: &str) {
    for instance_id in instance_ids {
        sqlx::query("DELETE FROM container_registry WHERE instance_id = $1")
            .bind(instance_id)
            .execute(pool)
            .await
            .ok();
        sqlx::query("DELETE FROM instance_images WHERE instance_id = $1")
            .bind(instance_id)
            .execute(pool)
            .await
            .ok();
        sqlx::query("DELETE FROM instances WHERE instance_id = $1")
            .bind(instance_id)
            .execute(pool)
            .await
            .ok();
    }
    sqlx::query("DELETE FROM images WHERE image_id = $1")
        .bind(image_id)
        .execute(pool)
        .await
        .ok();
}

#[tokio::test]
async fn test_cleanup_old_terminal_instances() {
    skip_if_no_db!();
    let _sweep = SWEEP_LOCK.lock().await;
    let pool = get_test_pool().await.expect("Failed to get test pool");
    let persistence = Arc::new(PostgresPersistence::new(pool.clone()));
    let tenant_id = format!("test-tenant-{}", Uuid::new_v4());
    let image_id = create_test_image(&pool, &tenant_id).await;

    // Create instances with different statuses
    let old_completed = Uuid::new_v4().to_string();
    let old_failed = Uuid::new_v4().to_string();
    let old_running = Uuid::new_v4().to_string();
    let recent_completed = Uuid::new_v4().to_string();

    let old_time = Utc::now() - ChronoDuration::days(35);
    let recent_time = Utc::now() - ChronoDuration::hours(1);

    // Old completed instance (should be deleted)
    create_test_instance(
        &pool,
        &old_completed,
        &tenant_id,
        &image_id,
        "completed",
        Some(old_time),
    )
    .await;
    create_instance_image(&pool, &old_completed, &image_id, &tenant_id).await;
    create_container_registry(&pool, &old_completed, &tenant_id).await;

    // Old failed instance (should be deleted)
    create_test_instance(
        &pool,
        &old_failed,
        &tenant_id,
        &image_id,
        "failed",
        Some(old_time),
    )
    .await;
    create_instance_image(&pool, &old_failed, &image_id, &tenant_id).await;

    // Old running instance (should NOT be deleted - not terminal)
    create_test_instance(&pool, &old_running, &tenant_id, &image_id, "running", None).await;
    create_instance_image(&pool, &old_running, &image_id, &tenant_id).await;

    // Recent completed instance (should NOT be deleted - too recent)
    create_test_instance(
        &pool,
        &recent_completed,
        &tenant_id,
        &image_id,
        "completed",
        Some(recent_time),
    )
    .await;
    create_instance_image(&pool, &recent_completed, &image_id, &tenant_id).await;

    // Create cleanup worker with 30-day max age
    let config = DbCleanupWorkerConfig {
        enabled: true,
        poll_interval: Duration::from_secs(1),
        max_age: Duration::from_secs(30 * 24 * 3600), // 30 days
        batch_size: 100,
        // Instance retention is what these cases exercise; the debug-event
        // sweep has its own window and is covered separately.
        debug_event_max_age: None,
    };
    let worker = DbCleanupWorker::new(pool.clone(), persistence, config);
    let shutdown = worker.shutdown_handle();

    // Run worker for a short time
    let handle = tokio::spawn(async move {
        worker.run().await;
    });

    // Wait for cleanup cycle
    tokio::time::sleep(Duration::from_millis(1500)).await;
    shutdown.notify_one();
    handle.await.expect("Worker task failed");

    // Verify old terminal instances were deleted
    assert!(
        !instance_exists(&pool, &old_completed).await,
        "Old completed instance should be deleted"
    );
    assert!(
        !instance_exists(&pool, &old_failed).await,
        "Old failed instance should be deleted"
    );

    // Verify environment tables were cleaned
    assert!(
        !instance_image_exists(&pool, &old_completed).await,
        "instance_images should be deleted"
    );
    assert!(
        !container_registry_exists(&pool, &old_completed).await,
        "container_registry should be deleted"
    );

    // Verify non-terminal and recent instances were NOT deleted
    assert!(
        instance_exists(&pool, &old_running).await,
        "Running instance should NOT be deleted"
    );
    assert!(
        instance_exists(&pool, &recent_completed).await,
        "Recent completed instance should NOT be deleted"
    );

    // Cleanup
    cleanup_test_data(&pool, &[&old_running, &recent_completed], &image_id).await;
}

#[tokio::test]
async fn test_cleanup_disabled_by_default() {
    skip_if_no_db!();
    let _sweep = SWEEP_LOCK.lock().await;
    let pool = get_test_pool().await.expect("Failed to get test pool");
    let persistence = Arc::new(PostgresPersistence::new(pool.clone()));
    let tenant_id = format!("test-tenant-{}", Uuid::new_v4());
    let image_id = create_test_image(&pool, &tenant_id).await;

    let old_completed = Uuid::new_v4().to_string();
    let old_time = Utc::now() - ChronoDuration::days(35);

    create_test_instance(
        &pool,
        &old_completed,
        &tenant_id,
        &image_id,
        "completed",
        Some(old_time),
    )
    .await;
    create_instance_image(&pool, &old_completed, &image_id, &tenant_id).await;

    // Create cleanup worker with cleanup DISABLED
    let config = DbCleanupWorkerConfig {
        enabled: false, // Disabled!
        poll_interval: Duration::from_secs(1),
        max_age: Duration::from_secs(30 * 24 * 3600),
        batch_size: 100,
        // Instance retention is what these cases exercise; the debug-event
        // sweep has its own window and is covered separately.
        debug_event_max_age: None,
    };
    let worker = DbCleanupWorker::new(pool.clone(), persistence, config);
    let shutdown = worker.shutdown_handle();

    let handle = tokio::spawn(async move {
        worker.run().await;
    });

    // Wait and then shutdown
    tokio::time::sleep(Duration::from_millis(500)).await;
    shutdown.notify_one();
    handle.await.expect("Worker task failed");

    // Instance should still exist (cleanup was disabled)
    assert!(
        instance_exists(&pool, &old_completed).await,
        "Instance should NOT be deleted when cleanup is disabled"
    );

    // Cleanup
    cleanup_test_data(&pool, &[&old_completed], &image_id).await;
}

#[test]
fn test_config_default() {
    // Test that the default config has expected values
    let config = DbCleanupWorkerConfig::default();

    assert!(config.enabled, "Should be enabled by default");
    assert_eq!(config.poll_interval, Duration::from_secs(3600));
    assert_eq!(config.max_age, Duration::from_secs(3 * 24 * 3600));
    assert_eq!(config.batch_size, 100);
}

#[test]
fn test_config_custom() {
    // Test that custom config values work correctly
    let config = DbCleanupWorkerConfig {
        enabled: true,
        poll_interval: Duration::from_secs(7200),
        max_age: Duration::from_secs(7 * 24 * 3600),
        batch_size: 50,
        // Instance retention is what these cases exercise; the debug-event
        // sweep has its own window and is covered separately.
        debug_event_max_age: None,
    };

    assert!(config.enabled);
    assert_eq!(config.poll_interval, Duration::from_secs(7200));
    assert_eq!(config.max_age, Duration::from_secs(7 * 24 * 3600));
    assert_eq!(config.batch_size, 50);
}

// =============================================================================
// E2E Tests - Full database integration
// =============================================================================

/// Create a checkpoint for an instance
async fn create_checkpoint(pool: &PgPool, instance_id: &str, checkpoint_id: &str) {
    sqlx::query(
        r#"
        INSERT INTO checkpoints (instance_id, checkpoint_id, state, created_at)
        VALUES ($1, $2, $3, NOW())
        "#,
    )
    .bind(instance_id)
    .bind(checkpoint_id)
    .bind(b"test-state".as_slice())
    .execute(pool)
    .await
    .expect("Failed to create checkpoint");
}

/// Create an event for an instance
async fn create_event(pool: &PgPool, instance_id: &str, event_type: &str) {
    sqlx::query(
        r#"
        INSERT INTO instance_events (instance_id, event_type, created_at)
        VALUES ($1, $2::instance_event_type, NOW())
        "#,
    )
    .bind(instance_id)
    .bind(event_type)
    .execute(pool)
    .await
    .expect("Failed to create event");
}

/// Check if checkpoints exist for an instance
async fn checkpoints_exist(pool: &PgPool, instance_id: &str) -> bool {
    let result: Option<(i32,)> =
        sqlx::query_as("SELECT 1 FROM checkpoints WHERE instance_id = $1 LIMIT 1")
            .bind(instance_id)
            .fetch_optional(pool)
            .await
            .expect("Failed to query checkpoints");
    result.is_some()
}

/// Check if events exist for an instance
async fn events_exist(pool: &PgPool, instance_id: &str) -> bool {
    let result: Option<(i32,)> =
        sqlx::query_as("SELECT 1 FROM instance_events WHERE instance_id = $1 LIMIT 1")
            .bind(instance_id)
            .fetch_optional(pool)
            .await
            .expect("Failed to query events");
    result.is_some()
}

/// Count instances in the database for a tenant
async fn count_instances(pool: &PgPool, tenant_id: &str) -> i64 {
    let result: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM instances WHERE tenant_id = $1")
        .bind(tenant_id)
        .fetch_one(pool)
        .await
        .expect("Failed to count instances");
    result.0
}

#[tokio::test]
async fn test_e2e_cascade_deletion_checkpoints_and_events() {
    skip_if_no_db!();
    let _sweep = SWEEP_LOCK.lock().await;
    let pool = get_test_pool().await.expect("Failed to get test pool");
    let persistence = Arc::new(PostgresPersistence::new(pool.clone()));
    let tenant_id = format!("test-tenant-{}", Uuid::new_v4());
    let image_id = create_test_image(&pool, &tenant_id).await;

    let instance_id = Uuid::new_v4().to_string();
    let old_time = Utc::now() - ChronoDuration::days(35);

    // Create instance with checkpoints and events
    create_test_instance(
        &pool,
        &instance_id,
        &tenant_id,
        &image_id,
        "completed",
        Some(old_time),
    )
    .await;
    create_instance_image(&pool, &instance_id, &image_id, &tenant_id).await;

    // Create multiple checkpoints
    create_checkpoint(&pool, &instance_id, "checkpoint-1").await;
    create_checkpoint(&pool, &instance_id, "checkpoint-2").await;
    create_checkpoint(&pool, &instance_id, "checkpoint-3").await;

    // Create multiple events
    create_event(&pool, &instance_id, "started").await;
    create_event(&pool, &instance_id, "progress").await;
    create_event(&pool, &instance_id, "completed").await;

    // Verify data was created
    assert!(instance_exists(&pool, &instance_id).await);
    assert!(checkpoints_exist(&pool, &instance_id).await);
    assert!(events_exist(&pool, &instance_id).await);

    // Run cleanup
    let config = DbCleanupWorkerConfig {
        enabled: true,
        poll_interval: Duration::from_secs(1),
        max_age: Duration::from_secs(30 * 24 * 3600),
        batch_size: 100,
        // Instance retention is what these cases exercise; the debug-event
        // sweep has its own window and is covered separately.
        debug_event_max_age: None,
    };
    let worker = DbCleanupWorker::new(pool.clone(), persistence, config);
    let shutdown = worker.shutdown_handle();

    let handle = tokio::spawn(async move {
        worker.run().await;
    });

    tokio::time::sleep(Duration::from_millis(1500)).await;
    shutdown.notify_one();
    handle.await.expect("Worker task failed");

    // Verify instance and ALL related data were deleted (CASCADE)
    assert!(
        !instance_exists(&pool, &instance_id).await,
        "Instance should be deleted"
    );
    assert!(
        !checkpoints_exist(&pool, &instance_id).await,
        "Checkpoints should be cascade deleted"
    );
    assert!(
        !events_exist(&pool, &instance_id).await,
        "Events should be cascade deleted"
    );
    assert!(
        !instance_image_exists(&pool, &instance_id).await,
        "instance_images should be deleted"
    );

    // Cleanup
    cleanup_test_data(&pool, &[], &image_id).await;
}

#[tokio::test]
async fn test_e2e_batch_processing() {
    skip_if_no_db!();
    let _sweep = SWEEP_LOCK.lock().await;
    let pool = get_test_pool().await.expect("Failed to get test pool");
    let persistence = Arc::new(PostgresPersistence::new(pool.clone()));
    let tenant_id = format!("test-tenant-{}", Uuid::new_v4());
    let image_id = create_test_image(&pool, &tenant_id).await;

    let old_time = Utc::now() - ChronoDuration::days(35);
    let batch_size = 3i64;
    let total_instances = 10;

    // Create more instances than batch size
    let mut instance_ids = Vec::new();
    for _ in 0..total_instances {
        let instance_id = Uuid::new_v4().to_string();
        create_test_instance(
            &pool,
            &instance_id,
            &tenant_id,
            &image_id,
            "completed",
            Some(old_time),
        )
        .await;
        create_instance_image(&pool, &instance_id, &image_id, &tenant_id).await;
        instance_ids.push(instance_id);
    }

    // Verify all instances exist
    assert_eq!(
        count_instances(&pool, &tenant_id).await,
        total_instances as i64
    );

    // Run cleanup with small batch size
    let config = DbCleanupWorkerConfig {
        enabled: true,
        poll_interval: Duration::from_secs(1),
        max_age: Duration::from_secs(30 * 24 * 3600),
        batch_size,
        // Instance retention is what these cases exercise; the debug-event
        // sweep has its own window and is covered separately.
        debug_event_max_age: None,
    };
    let worker = DbCleanupWorker::new(pool.clone(), persistence, config);
    let shutdown = worker.shutdown_handle();

    let handle = tokio::spawn(async move {
        worker.run().await;
    });

    // Wait long enough for multiple batches
    tokio::time::sleep(Duration::from_millis(2000)).await;
    shutdown.notify_one();
    handle.await.expect("Worker task failed");

    // All instances should be deleted (processed in batches)
    assert_eq!(
        count_instances(&pool, &tenant_id).await,
        0,
        "All instances should be deleted via batching"
    );

    // Cleanup
    cleanup_test_data(&pool, &[], &image_id).await;
}

#[tokio::test]
async fn test_e2e_cancelled_instances_deleted() {
    skip_if_no_db!();
    let _sweep = SWEEP_LOCK.lock().await;
    let pool = get_test_pool().await.expect("Failed to get test pool");
    let persistence = Arc::new(PostgresPersistence::new(pool.clone()));
    let tenant_id = format!("test-tenant-{}", Uuid::new_v4());
    let image_id = create_test_image(&pool, &tenant_id).await;

    let old_cancelled = Uuid::new_v4().to_string();
    let old_time = Utc::now() - ChronoDuration::days(35);

    // Create old cancelled instance (should be deleted - cancelled is terminal)
    create_test_instance(
        &pool,
        &old_cancelled,
        &tenant_id,
        &image_id,
        "cancelled",
        Some(old_time),
    )
    .await;
    create_instance_image(&pool, &old_cancelled, &image_id, &tenant_id).await;

    assert!(instance_exists(&pool, &old_cancelled).await);

    // Run cleanup
    let config = DbCleanupWorkerConfig {
        enabled: true,
        poll_interval: Duration::from_secs(1),
        max_age: Duration::from_secs(30 * 24 * 3600),
        batch_size: 100,
        // Instance retention is what these cases exercise; the debug-event
        // sweep has its own window and is covered separately.
        debug_event_max_age: None,
    };
    let worker = DbCleanupWorker::new(pool.clone(), persistence, config);
    let shutdown = worker.shutdown_handle();

    let handle = tokio::spawn(async move {
        worker.run().await;
    });

    tokio::time::sleep(Duration::from_millis(1500)).await;
    shutdown.notify_one();
    handle.await.expect("Worker task failed");

    // Cancelled instance should be deleted
    assert!(
        !instance_exists(&pool, &old_cancelled).await,
        "Old cancelled instance should be deleted"
    );

    // Cleanup
    cleanup_test_data(&pool, &[], &image_id).await;
}

#[tokio::test]
async fn test_e2e_suspended_instances_not_deleted() {
    skip_if_no_db!();
    let _sweep = SWEEP_LOCK.lock().await;
    let pool = get_test_pool().await.expect("Failed to get test pool");
    let persistence = Arc::new(PostgresPersistence::new(pool.clone()));
    let tenant_id = format!("test-tenant-{}", Uuid::new_v4());
    let image_id = create_test_image(&pool, &tenant_id).await;

    let old_suspended = Uuid::new_v4().to_string();

    // Create old suspended instance (should NOT be deleted - not terminal)
    sqlx::query(
        r#"
        INSERT INTO instances (instance_id, tenant_id, status, created_at, started_at, sleep_until)
        VALUES ($1, $2, 'suspended', NOW() - INTERVAL '40 days', NOW() - INTERVAL '40 days', NOW() + INTERVAL '1 day')
        "#,
    )
    .bind(&old_suspended)
    .bind(&tenant_id)
    .execute(&pool)
    .await
    .expect("Failed to create suspended instance");

    create_instance_image(&pool, &old_suspended, &image_id, &tenant_id).await;

    assert!(instance_exists(&pool, &old_suspended).await);

    // Run cleanup
    let config = DbCleanupWorkerConfig {
        enabled: true,
        poll_interval: Duration::from_secs(1),
        max_age: Duration::from_secs(30 * 24 * 3600),
        batch_size: 100,
        // Instance retention is what these cases exercise; the debug-event
        // sweep has its own window and is covered separately.
        debug_event_max_age: None,
    };
    let worker = DbCleanupWorker::new(pool.clone(), persistence, config);
    let shutdown = worker.shutdown_handle();

    let handle = tokio::spawn(async move {
        worker.run().await;
    });

    tokio::time::sleep(Duration::from_millis(1500)).await;
    shutdown.notify_one();
    handle.await.expect("Worker task failed");

    // Suspended instance should NOT be deleted (not a terminal state)
    assert!(
        instance_exists(&pool, &old_suspended).await,
        "Suspended instance should NOT be deleted"
    );

    // Cleanup
    cleanup_test_data(&pool, &[&old_suspended], &image_id).await;
}

#[tokio::test]
async fn test_e2e_pending_instances_not_deleted() {
    skip_if_no_db!();
    let _sweep = SWEEP_LOCK.lock().await;
    let pool = get_test_pool().await.expect("Failed to get test pool");
    let persistence = Arc::new(PostgresPersistence::new(pool.clone()));
    let tenant_id = format!("test-tenant-{}", Uuid::new_v4());
    let image_id = create_test_image(&pool, &tenant_id).await;

    let old_pending = Uuid::new_v4().to_string();

    // Create old pending instance (should NOT be deleted - not terminal)
    sqlx::query(
        r#"
        INSERT INTO instances (instance_id, tenant_id, status, created_at)
        VALUES ($1, $2, 'pending', NOW() - INTERVAL '40 days')
        "#,
    )
    .bind(&old_pending)
    .bind(&tenant_id)
    .execute(&pool)
    .await
    .expect("Failed to create pending instance");

    create_instance_image(&pool, &old_pending, &image_id, &tenant_id).await;

    assert!(instance_exists(&pool, &old_pending).await);

    // Run cleanup
    let config = DbCleanupWorkerConfig {
        enabled: true,
        poll_interval: Duration::from_secs(1),
        max_age: Duration::from_secs(30 * 24 * 3600),
        batch_size: 100,
        // Instance retention is what these cases exercise; the debug-event
        // sweep has its own window and is covered separately.
        debug_event_max_age: None,
    };
    let worker = DbCleanupWorker::new(pool.clone(), persistence, config);
    let shutdown = worker.shutdown_handle();

    let handle = tokio::spawn(async move {
        worker.run().await;
    });

    tokio::time::sleep(Duration::from_millis(1500)).await;
    shutdown.notify_one();
    handle.await.expect("Worker task failed");

    // Pending instance should NOT be deleted (not a terminal state)
    assert!(
        instance_exists(&pool, &old_pending).await,
        "Pending instance should NOT be deleted"
    );

    // Cleanup
    cleanup_test_data(&pool, &[&old_pending], &image_id).await;
}

/// Insert an event with an explicit age and subtype.
async fn create_aged_event(
    pool: &PgPool,
    instance_id: &str,
    event_type: &str,
    subtype: Option<&str>,
    hours_ago: i64,
) {
    sqlx::query(
        r#"
        INSERT INTO instance_events (instance_id, event_type, subtype, created_at)
        VALUES ($1, $2::instance_event_type, $3, NOW() - ($4 || ' hours')::interval)
        "#,
    )
    .bind(instance_id)
    .bind(event_type)
    .bind(subtype)
    .bind(hours_ago.to_string())
    .execute(pool)
    .await
    .expect("Failed to create aged event");
}

async fn count_events(pool: &PgPool, instance_id: &str, subtype: Option<&str>) -> i64 {
    let (n,): (i64,) = match subtype {
        Some(sub) => sqlx::query_as(
            "SELECT COUNT(*) FROM instance_events WHERE instance_id = $1 AND subtype = $2",
        )
        .bind(instance_id)
        .bind(sub),
        None => sqlx::query_as(
            "SELECT COUNT(*) FROM instance_events WHERE instance_id = $1 AND subtype IS NULL",
        )
        .bind(instance_id),
    }
    .fetch_one(pool)
    .await
    .expect("Failed to count events");
    n
}

/// Step-debug events age out on their own, shorter window; everything else
/// waits for its instance.
///
/// Debug payloads dominate `instance_events` — a burst that drains a large
/// sleeping population writes millions of them — but they are only read while a
/// run is recent. Ageing them separately keeps the table bounded without
/// reducing what workflows record, and without touching the lifecycle events
/// that are the run's durable history.
#[tokio::test]
async fn debug_events_age_out_before_their_instance_does() {
    skip_if_no_db!();
    let _sweep = SWEEP_LOCK.lock().await;
    let pool = get_test_pool().await.expect("test pool");
    let persistence = Arc::new(PostgresPersistence::new(pool.clone()));

    let tenant_id = format!("debug-sweep-{}", Uuid::new_v4());
    let image_id = create_test_image(&pool, &tenant_id).await;
    // Still running, so instance retention can never be what removes these.
    let instance_id = Uuid::new_v4().to_string();
    create_test_instance(&pool, &instance_id, &tenant_id, &image_id, "running", None).await;
    create_instance_image(&pool, &instance_id, &image_id, &tenant_id).await;

    create_aged_event(&pool, &instance_id, "custom", Some("step_debug_start"), 48).await;
    create_aged_event(&pool, &instance_id, "custom", Some("step_debug_end"), 48).await;
    create_aged_event(&pool, &instance_id, "custom", Some("step_debug_start"), 1).await;
    create_aged_event(&pool, &instance_id, "custom", Some("workflow_log"), 48).await;
    create_aged_event(&pool, &instance_id, "completed", None, 48).await;

    let config = DbCleanupWorkerConfig {
        enabled: true,
        poll_interval: Duration::from_secs(1),
        // Far longer than anything here: the instance sweep must not be what
        // removes the debug rows.
        max_age: Duration::from_secs(365 * 24 * 3600),
        batch_size: 100,
        debug_event_max_age: Some(Duration::from_secs(24 * 3600)),
    };
    let worker = DbCleanupWorker::new(pool.clone(), persistence, config);
    let shutdown = worker.shutdown_handle();
    let handle = tokio::spawn(async move { worker.run().await });

    tokio::time::sleep(Duration::from_secs(2)).await;
    shutdown.notify_one();
    let _ = handle.await;

    assert_eq!(
        count_events(&pool, &instance_id, Some("step_debug_start")).await,
        1,
        "the aged step_debug_start must go and the recent one must stay"
    );
    assert_eq!(
        count_events(&pool, &instance_id, Some("step_debug_end")).await,
        0,
        "the aged step_debug_end must go"
    );
    assert_eq!(
        count_events(&pool, &instance_id, Some("workflow_log")).await,
        1,
        "non-debug custom events are not the sweep's business"
    );
    assert_eq!(
        count_events(&pool, &instance_id, None).await,
        1,
        "lifecycle events are the run's history and must survive"
    );
    let (still_there,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM instances WHERE instance_id = $1")
            .bind(&instance_id)
            .fetch_one(&pool)
            .await
            .expect("Failed to query instance");
    assert_eq!(still_there, 1, "the instance itself must be untouched");

    cleanup_test_data(&pool, &[&instance_id], &image_id).await;
}

/// `debug_event_max_age: None` must leave every debug event alone.
#[tokio::test]
async fn debug_sweep_is_off_when_no_window_is_configured() {
    skip_if_no_db!();
    let _sweep = SWEEP_LOCK.lock().await;
    let pool = get_test_pool().await.expect("test pool");
    let persistence = Arc::new(PostgresPersistence::new(pool.clone()));

    let tenant_id = format!("debug-sweep-off-{}", Uuid::new_v4());
    let image_id = create_test_image(&pool, &tenant_id).await;
    let instance_id = Uuid::new_v4().to_string();
    create_test_instance(&pool, &instance_id, &tenant_id, &image_id, "running", None).await;
    create_instance_image(&pool, &instance_id, &image_id, &tenant_id).await;
    create_aged_event(&pool, &instance_id, "custom", Some("step_debug_start"), 999).await;

    let config = DbCleanupWorkerConfig {
        enabled: true,
        poll_interval: Duration::from_secs(1),
        max_age: Duration::from_secs(365 * 24 * 3600),
        batch_size: 100,
        debug_event_max_age: None,
    };
    let worker = DbCleanupWorker::new(pool.clone(), persistence, config);
    let shutdown = worker.shutdown_handle();
    let handle = tokio::spawn(async move { worker.run().await });
    tokio::time::sleep(Duration::from_secs(2)).await;
    shutdown.notify_one();
    let _ = handle.await;

    assert_eq!(
        count_events(&pool, &instance_id, Some("step_debug_start")).await,
        1,
        "with no window configured the sweep must not run at all"
    );

    cleanup_test_data(&pool, &[&instance_id], &image_id).await;
}

/// A finished child of a running parent stays; one pass over a large pinned
/// population reads each pinned row once (the cursor), counts it once, and
/// still deletes what is eligible around it.
#[tokio::test]
async fn pinned_children_are_read_once_per_pass() {
    skip_if_no_db!();
    let _sweep = SWEEP_LOCK.lock().await;
    const PINNED: i64 = 100_000;
    let pool = get_test_pool().await.expect("Failed to get test pool");
    let persistence = Arc::new(PostgresPersistence::new(pool.clone()));
    let tenant_id = format!("pinned-{}", Uuid::new_v4());
    let parent = format!("{tenant_id}-parent");
    let ended_parent = format!("{tenant_id}-ended-parent");
    create_test_instance(&pool, &parent, &tenant_id, "", "running", None).await;
    create_test_instance(
        &pool,
        &ended_parent,
        &tenant_id,
        "",
        "completed",
        Some(Utc::now() - ChronoDuration::days(35)),
    )
    .await;
    // 100k finished children of the running parent, 40 days old.
    sqlx::query(
        r#"
        INSERT INTO instances (instance_id, tenant_id, status, created_at, finished_at,
                               parent_instance_id, parent_close_policy, admitted_at)
        SELECT $1 || '-pinned-' || g, $1, 'completed', NOW() - INTERVAL '41 days',
               NOW() - INTERVAL '40 days' + g * INTERVAL '1 millisecond', $2, 'cancel',
               NOW() - INTERVAL '41 days'
        FROM generate_series(1, $3) AS g
        "#,
    )
    .bind(&tenant_id)
    .bind(&parent)
    .bind(PINNED)
    .execute(&pool)
    .await
    .expect("seed pinned children");
    // A bulk seed leaves the planner's statistics describing the table before
    // it; production grows gradually and autovacuum keeps up.
    sqlx::query("ANALYZE instances")
        .execute(&pool)
        .await
        .unwrap();
    // Eligible rows spread through the pinned range, and a child of an old,
    // ended parent.
    let mut eligible = Vec::new();
    for i in 0..25 {
        let id = format!("{tenant_id}-plain-{i}");
        create_test_instance(
            &pool,
            &id,
            &tenant_id,
            "",
            "failed",
            Some(Utc::now() - ChronoDuration::days(40) + ChronoDuration::seconds(i * 4)),
        )
        .await;
        eligible.push(id);
    }
    let released = format!("{tenant_id}-released");
    sqlx::query(
        "INSERT INTO instances (instance_id, tenant_id, status, created_at, finished_at, \
                                parent_instance_id, parent_close_policy, admitted_at) \
         VALUES ($1, $2, 'completed', NOW() - INTERVAL '41 days', NOW() - INTERVAL '39 days', \
                 $3, 'cancel', NOW() - INTERVAL '41 days')",
    )
    .bind(&released)
    .bind(&tenant_id)
    .bind(&ended_parent)
    .execute(&pool)
    .await
    .unwrap();
    eligible.push(released);

    // Every pinned row in the database, not only this test's.
    let expected_pinned: i64 = sqlx::query_scalar(
        r#"
        SELECT count(*) FROM instances AS i
        JOIN instances AS p ON p.instance_id = i.parent_instance_id AND p.tenant_id = i.tenant_id
        WHERE i.status IN ('completed', 'failed', 'cancelled')
          AND i.finished_at < NOW() - INTERVAL '30 days'
          AND NOT (p.status IN ('completed', 'failed', 'cancelled')
                   AND p.finished_at < NOW() - INTERVAL '30 days')
        "#,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(expected_pinned >= PINNED);

    let worker = DbCleanupWorker::new(
        pool.clone(),
        persistence,
        DbCleanupWorkerConfig {
            enabled: true,
            poll_interval: Duration::from_secs(3600),
            max_age: Duration::from_secs(30 * 24 * 3600),
            batch_size: 1000,
            debug_event_max_age: None,
        },
    );
    let started = std::time::Instant::now();
    let stats = worker.run_once().await.expect("retention pass");
    eprintln!("pinned pass: {stats:?} in {:?}", started.elapsed());
    assert_eq!(
        stats.pinned_terminal_children as i64, expected_pinned,
        "each pinned row is read exactly once per pass"
    );
    assert!(
        stats.pages as i64 <= (expected_pinned + stats.deleted as i64) / 1000 + 2,
        "the pass pages forward instead of re-reading: {stats:?}"
    );
    for id in &eligible {
        assert!(!instance_exists(&pool, id).await, "{id} should be deleted");
    }
    let remaining: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM instances WHERE tenant_id = $1 AND parent_instance_id = $2",
    )
    .bind(&tenant_id)
    .bind(&parent)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        remaining, PINNED,
        "pinned children stay while the parent runs"
    );
    assert!(instance_exists(&pool, &parent).await);

    sqlx::query("DELETE FROM instances WHERE tenant_id = $1")
        .bind(&tenant_id)
        .execute(&pool)
        .await
        .unwrap();
}

/// Images held by a launch generation (ON DELETE RESTRICT) are skipped, so
/// a batch full of them no longer starves deletable images behind them.
#[tokio::test]
async fn images_held_by_launches_do_not_starve_cleanup() {
    use runtara_environment::image_cleanup_worker::{ImageCleanupWorker, ImageCleanupWorkerConfig};
    skip_if_no_db!();
    let _sweep = SWEEP_LOCK.lock().await;
    let pool = get_test_pool().await.expect("Failed to get test pool");
    let tenant_id = format!("image-starve-{}", Uuid::new_v4());
    let mut held = Vec::new();
    for i in 0..5 {
        let image_id = create_test_image(&pool, &tenant_id).await;
        sqlx::query("UPDATE images SET updated_at = TIMESTAMPTZ '2000-01-01' + $2 * INTERVAL '1 minute' WHERE image_id = $1")
            .bind(&image_id)
            .bind(i)
            .execute(&pool)
            .await
            .unwrap();
        let instance_id = format!("{tenant_id}-held-{i}");
        create_test_instance(
            &pool,
            &instance_id,
            &tenant_id,
            &image_id,
            "completed",
            Some(Utc::now() - ChronoDuration::days(40)),
        )
        .await;
        sqlx::query(
            "INSERT INTO instance_launches (launch_id, instance_id, tenant_id, image_id, kind, \
                                            state, deadline_at) \
             VALUES ($1, $2, $3, $4, 'start', 'completed', NOW())",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(&instance_id)
        .bind(&tenant_id)
        .bind(&image_id)
        .execute(&pool)
        .await
        .unwrap();
        held.push((image_id, instance_id));
    }
    let free = create_test_image(&pool, &tenant_id).await;
    sqlx::query("UPDATE images SET updated_at = TIMESTAMPTZ '2000-01-02' WHERE image_id = $1")
        .bind(&free)
        .execute(&pool)
        .await
        .unwrap();

    let data_dir = tempfile::tempdir().unwrap();
    let worker = ImageCleanupWorker::new(
        pool.clone(),
        ImageCleanupWorkerConfig {
            batch_size: 5,
            data_dir: data_dir.path().to_path_buf(),
            ..Default::default()
        },
    );
    worker.run_once().await.expect("image cleanup");
    let exists = |image: String| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM images WHERE image_id = $1)")
                .bind(image)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    assert!(
        !exists(free.clone()).await,
        "the deletable image behind them is removed"
    );
    for (image, _) in &held {
        assert!(
            exists(image.clone()).await,
            "an image held by a launch stays"
        );
    }

    sqlx::query("DELETE FROM instances WHERE tenant_id = $1")
        .bind(&tenant_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM images WHERE tenant_id = $1")
        .bind(&tenant_id)
        .execute(&pool)
        .await
        .unwrap();
}

/// status, input, output, stderr, run label, parent, checkpoints, custom signals.
type PrunedRow = (
    String,
    Option<Vec<u8>>,
    Option<Vec<u8>>,
    Option<String>,
    Option<String>,
    Option<String>,
    i64,
    i64,
);

/// Slice 12: after deleting what retention may delete, a pass prunes the
/// pinned terminal children it had to keep (terminal and past retention by
/// their own finish, parent still live). Their row and outcome stay; their
/// checkpoints, custom signals, `input` and `stderr` go. A child of an old,
/// ended parent is deleted, not pruned; a recent child and a running child
/// are untouched. The prune pages with the batch-size cursor, and a rerun
/// prunes nothing.
#[tokio::test]
async fn pinned_children_are_pruned_after_deletion_with_a_cursor() {
    skip_if_no_db!();
    let _sweep = SWEEP_LOCK.lock().await;
    const PINNED: usize = 5;
    let pool = get_test_pool().await.expect("Failed to get test pool");
    let persistence = Arc::new(PostgresPersistence::new(pool.clone()));
    let tenant_id = format!("prune-{}", Uuid::new_v4());
    let parent = format!("{tenant_id}-parent");
    let ended_parent = format!("{tenant_id}-ended-parent");
    create_test_instance(&pool, &parent, &tenant_id, "", "running", None).await;
    create_test_instance(
        &pool,
        &ended_parent,
        &tenant_id,
        "",
        "completed",
        Some(Utc::now() - ChronoDuration::days(35)),
    )
    .await;
    let child = |id: &str, parent: &str, status: &str, finished_days: Option<i64>| {
        let (pool, tenant_id) = (pool.clone(), tenant_id.clone());
        let (id, parent, status) = (id.to_owned(), parent.to_owned(), status.to_owned());
        async move {
            sqlx::query(
                "INSERT INTO instances (instance_id, tenant_id, status, created_at, finished_at, \
                                        parent_instance_id, parent_close_policy, admitted_at, \
                                        input, output, stderr, run_label) \
                 VALUES ($1, $2, $3::instance_status, NOW() - INTERVAL '41 days', \
                         NOW() - make_interval(days => $5::int), $4, 'cancel', \
                         NOW() - INTERVAL '41 days', 'bulky input'::bytea, \
                         '{\"answer\":42}'::bytea, 'noisy stderr', 'label')",
            )
            .bind(&id)
            .bind(&tenant_id)
            .bind(&status)
            .bind(&parent)
            .bind(finished_days.map(|d| d as i32))
            .execute(&pool)
            .await
            .expect("seed child");
            sqlx::query(
                "INSERT INTO pending_checkpoint_signals (instance_id, checkpoint_id, payload) \
                 VALUES ($1, 'raw', 'payload'::bytea)",
            )
            .bind(&id)
            .execute(&pool)
            .await
            .expect("seed custom signal");
            create_checkpoint(&pool, &id, "cp-1").await;
            id
        }
    };
    let mut pinned = Vec::new();
    for i in 0..PINNED {
        pinned.push(
            child(
                &format!("{tenant_id}-pinned-{i}"),
                &parent,
                "completed",
                Some(40),
            )
            .await,
        );
    }
    let released = child(
        &format!("{tenant_id}-released"),
        &ended_parent,
        "failed",
        Some(39),
    )
    .await;
    let fresh = child(&format!("{tenant_id}-fresh"), &parent, "completed", Some(1)).await;
    let busy = child(&format!("{tenant_id}-busy"), &parent, "running", None).await;

    let worker = DbCleanupWorker::new(
        pool.clone(),
        persistence,
        DbCleanupWorkerConfig {
            enabled: true,
            poll_interval: Duration::from_secs(3600),
            max_age: Duration::from_secs(30 * 24 * 3600),
            batch_size: 2,
            debug_event_max_age: None,
        },
    );
    let stats = worker.run_once().await.expect("retention pass");
    eprintln!("prune pass: {stats:?}");

    // Deletion first: the child of the old, ended parent is gone, not pruned.
    assert!(!instance_exists(&pool, &released).await);
    assert!(stats.deleted >= 1);
    // The pinned children are still pinned for deletion, and pruned.
    assert!(stats.pinned_terminal_children >= PINNED as u64);
    assert!(stats.pruned_children >= PINNED as u64, "{stats:?}");
    assert!(
        stats.prune_pages >= PINNED.div_ceil(2) as u64,
        "a batch size of 2 pages the prune: {stats:?}"
    );
    let row = |id: String| {
        let pool = pool.clone();
        async move {
            let row: PrunedRow =
                sqlx::query_as(
                    "SELECT status::text, input, output, stderr, run_label, parent_instance_id, \
                            (SELECT count(*) FROM checkpoints c WHERE c.instance_id = i.instance_id), \
                            (SELECT count(*) FROM pending_checkpoint_signals s WHERE s.instance_id = i.instance_id) \
                     FROM instances i WHERE instance_id = $1",
                )
                .bind(&id)
                .fetch_one(&pool)
                .await
                .expect("row");
            row
        }
    };
    for id in &pinned {
        let (status, input, output, stderr, label, parent_id, checkpoints, signals) =
            row(id.clone()).await;
        assert_eq!(status, "completed");
        assert_eq!(
            output.as_deref(),
            Some(br#"{"answer":42}"#.as_slice()),
            "{id}: outcome kept"
        );
        assert_eq!(label.as_deref(), Some("label"));
        assert_eq!(parent_id.as_deref(), Some(parent.as_str()));
        assert!(
            input.is_none() && stderr.is_none(),
            "{id}: input and stderr cleared"
        );
        assert_eq!(
            (checkpoints, signals),
            (0, 0),
            "{id}: checkpoints and signals dropped"
        );
    }
    for id in [&fresh, &busy] {
        let (_, input, _, stderr, _, _, checkpoints, signals) = row(id.clone()).await;
        assert!(input.is_some() && stderr.is_some(), "{id} is not pruned");
        assert_eq!((checkpoints, signals), (1, 1), "{id} is not pruned");
    }

    // A rerun prunes nothing and changes nothing.
    let again = worker.run_once().await.expect("second pass");
    assert_eq!(again.pruned_children, 0, "{again:?}");
    let (status, _, output, ..) = row(pinned[0].clone()).await;
    assert_eq!(status, "completed");
    assert!(output.is_some());

    sqlx::query("DELETE FROM instances WHERE tenant_id = $1")
        .bind(&tenant_id)
        .execute(&pool)
        .await
        .unwrap();
}
