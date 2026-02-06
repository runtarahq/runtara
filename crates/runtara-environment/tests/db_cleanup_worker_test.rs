// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Tests for db_cleanup_worker module - cleaning up old database records.

mod common;

use chrono::{Duration as ChronoDuration, Utc};
use runtara_core::persistence::PostgresPersistence;
use runtara_environment::db_cleanup_worker::{DbCleanupWorker, DbCleanupWorkerConfig};
use sqlx::PgPool;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

/// Helper macro to skip tests if database URL is not set.
macro_rules! skip_if_no_db {
    () => {
        if std::env::var("TEST_ENVIRONMENT_DATABASE_URL").is_err()
            && std::env::var("RUNTARA_ENVIRONMENT_DATABASE_URL").is_err()
        {
            eprintln!(
                "Skipping test: TEST_ENVIRONMENT_DATABASE_URL or RUNTARA_ENVIRONMENT_DATABASE_URL not set"
            );
            return;
        }
    };
}

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Get a database pool for testing
async fn get_test_pool() -> Option<PgPool> {
    let database_url = std::env::var("TEST_ENVIRONMENT_DATABASE_URL")
        .or_else(|_| std::env::var("RUNTARA_ENVIRONMENT_DATABASE_URL"))
        .ok()?;
    let pool = PgPool::connect(&database_url).await.ok()?;
    MIGRATOR.run(&pool).await.ok()?;
    Some(pool)
}

/// Create a test image in the database with a unique name
async fn create_test_image(pool: &PgPool, tenant_id: &str) -> String {
    let image_id = Uuid::new_v4().to_string();
    let image_name = format!("test-image-{}", image_id);
    sqlx::query(
        r#"
        INSERT INTO images (image_id, tenant_id, name, description, binary_path, bundle_path, runner_type)
        VALUES ($1, $2, $3, 'Test image', '/usr/bin/test', '/tmp/test-bundle', 'mock')
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
    image_id: &str,
    status: &str,
    finished_at: Option<chrono::DateTime<Utc>>,
) {
    sqlx::query(
        r#"
        INSERT INTO instances (instance_id, tenant_id, image_id, status, created_at, started_at, finished_at)
        VALUES ($1, $2, $3, $4, NOW() - INTERVAL '2 days', NOW() - INTERVAL '2 days', $5)
        "#,
    )
    .bind(instance_id)
    .bind(tenant_id)
    .bind(image_id)
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
        INSERT INTO container_registry (container_id, instance_id, tenant_id, binary_path)
        VALUES ($1, $2, $3, '/usr/bin/test')
        ON CONFLICT (instance_id) DO UPDATE SET updated_at = NOW()
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
    let result: Option<(i64,)> = sqlx::query_as("SELECT 1 FROM instances WHERE instance_id = $1")
        .bind(instance_id)
        .fetch_optional(pool)
        .await
        .expect("Failed to query instance");
    result.is_some()
}

/// Check if an instance_images entry exists
async fn instance_image_exists(pool: &PgPool, instance_id: &str) -> bool {
    let result: Option<(i64,)> =
        sqlx::query_as("SELECT 1 FROM instance_images WHERE instance_id = $1")
            .bind(instance_id)
            .fetch_optional(pool)
            .await
            .expect("Failed to query instance_images");
    result.is_some()
}

/// Check if a container_registry entry exists
async fn container_registry_exists(pool: &PgPool, instance_id: &str) -> bool {
    let result: Option<(i64,)> =
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

    assert!(!config.enabled, "Should be disabled by default for safety");
    assert_eq!(config.poll_interval, Duration::from_secs(3600));
    assert_eq!(config.max_age, Duration::from_secs(30 * 24 * 3600));
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
        VALUES ($1, $2, NOW())
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
    let result: Option<(i64,)> =
        sqlx::query_as("SELECT 1 FROM checkpoints WHERE instance_id = $1 LIMIT 1")
            .bind(instance_id)
            .fetch_optional(pool)
            .await
            .expect("Failed to query checkpoints");
    result.is_some()
}

/// Check if events exist for an instance
async fn events_exist(pool: &PgPool, instance_id: &str) -> bool {
    let result: Option<(i64,)> =
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
    let pool = get_test_pool().await.expect("Failed to get test pool");
    let persistence = Arc::new(PostgresPersistence::new(pool.clone()));
    let tenant_id = format!("test-tenant-{}", Uuid::new_v4());
    let image_id = create_test_image(&pool, &tenant_id).await;

    let old_suspended = Uuid::new_v4().to_string();

    // Create old suspended instance (should NOT be deleted - not terminal)
    sqlx::query(
        r#"
        INSERT INTO instances (instance_id, tenant_id, image_id, status, created_at, started_at, sleep_until)
        VALUES ($1, $2, $3, 'suspended', NOW() - INTERVAL '40 days', NOW() - INTERVAL '40 days', NOW() + INTERVAL '1 day')
        "#,
    )
    .bind(&old_suspended)
    .bind(&tenant_id)
    .bind(&image_id)
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
    let pool = get_test_pool().await.expect("Failed to get test pool");
    let persistence = Arc::new(PostgresPersistence::new(pool.clone()));
    let tenant_id = format!("test-tenant-{}", Uuid::new_v4());
    let image_id = create_test_image(&pool, &tenant_id).await;

    let old_pending = Uuid::new_v4().to_string();

    // Create old pending instance (should NOT be deleted - not terminal)
    sqlx::query(
        r#"
        INSERT INTO instances (instance_id, tenant_id, image_id, status, created_at)
        VALUES ($1, $2, $3, 'pending', NOW() - INTERVAL '40 days')
        "#,
    )
    .bind(&old_pending)
    .bind(&tenant_id)
    .bind(&image_id)
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
