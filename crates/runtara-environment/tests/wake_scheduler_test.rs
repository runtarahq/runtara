// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Tests for wake_scheduler module and related database operations.

mod common;

use chrono::{Duration as ChronoDuration, Utc};
use runtara_environment::db::{self, Instance};
use runtara_environment::runner::MockRunner;
use runtara_environment::wake_scheduler::{WakeScheduler, WakeSchedulerConfig};
use sqlx::PgPool;
use std::path::PathBuf;
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
async fn create_test_instance(pool: &PgPool, instance_id: &str, tenant_id: &str, image_id: &str) {
    sqlx::query(
        r#"
        INSERT INTO instances (instance_id, tenant_id, image_id, status, created_at)
        VALUES ($1, $2, $3, 'suspended', NOW())
        "#,
    )
    .bind(instance_id)
    .bind(tenant_id)
    .bind(image_id)
    .execute(pool)
    .await
    .expect("Failed to create test instance");
}

/// Clean up test data
async fn cleanup(pool: &PgPool, instance_id: &str) {
    sqlx::query("DELETE FROM wake_queue WHERE instance_id = $1")
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

async fn cleanup_image(pool: &PgPool, image_id: &str) {
    sqlx::query("DELETE FROM images WHERE image_id = $1")
        .bind(image_id)
        .execute(pool)
        .await
        .ok();
}

// ============================================================================
// WakeSchedulerConfig Tests (Unit tests - no DB required)
// ============================================================================

#[test]
fn test_wake_scheduler_config_default() {
    let config = WakeSchedulerConfig::default();
    assert_eq!(config.poll_interval, Duration::from_secs(5));
    assert_eq!(config.batch_size, 10);
    assert_eq!(config.core_addr, "127.0.0.1:8001");
    assert_eq!(config.data_dir, PathBuf::from(".data"));
}

#[test]
fn test_wake_scheduler_config_custom() {
    let config = WakeSchedulerConfig {
        poll_interval: Duration::from_secs(10),
        batch_size: 50,
        core_addr: "192.168.1.100:9000".to_string(),
        data_dir: PathBuf::from("/var/data"),
    };

    assert_eq!(config.poll_interval, Duration::from_secs(10));
    assert_eq!(config.batch_size, 50);
    assert_eq!(config.core_addr, "192.168.1.100:9000");
    assert_eq!(config.data_dir, PathBuf::from("/var/data"));
}

#[test]
fn test_wake_scheduler_config_clone() {
    let config = WakeSchedulerConfig {
        poll_interval: Duration::from_secs(15),
        batch_size: 25,
        core_addr: "test:1234".to_string(),
        data_dir: PathBuf::from("/test"),
    };

    let cloned = config.clone();
    assert_eq!(config.poll_interval, cloned.poll_interval);
    assert_eq!(config.batch_size, cloned.batch_size);
    assert_eq!(config.core_addr, cloned.core_addr);
    assert_eq!(config.data_dir, cloned.data_dir);
}

#[test]
fn test_wake_scheduler_config_debug() {
    let config = WakeSchedulerConfig::default();
    let debug_str = format!("{:?}", config);
    assert!(debug_str.contains("poll_interval"));
    assert!(debug_str.contains("batch_size"));
    assert!(debug_str.contains("core_addr"));
    assert!(debug_str.contains("data_dir"));
}

// ============================================================================
// WakeEntry Tests
// ============================================================================

#[test]
fn test_wake_entry_debug() {
    // We can't easily test WakeEntry creation without DB, but we can test its Debug impl
    // by understanding its structure.
    // WakeEntry has: instance_id, tenant_id, image_id (String), checkpoint_id, wake_at (DateTime)
    let entry = db::WakeEntry {
        instance_id: "inst-123".to_string(),
        tenant_id: "tenant-456".to_string(),
        image_id: Uuid::new_v4().to_string(),
        checkpoint_id: "cp-789".to_string(),
        wake_at: Utc::now(),
    };

    let debug_str = format!("{:?}", entry);
    assert!(debug_str.contains("inst-123"));
    assert!(debug_str.contains("tenant-456"));
    assert!(debug_str.contains("cp-789"));
}

#[test]
fn test_wake_entry_clone() {
    let entry = db::WakeEntry {
        instance_id: "inst-1".to_string(),
        tenant_id: "tenant-1".to_string(),
        image_id: Uuid::new_v4().to_string(),
        checkpoint_id: "cp-1".to_string(),
        wake_at: Utc::now(),
    };

    let cloned = entry.clone();
    assert_eq!(entry.instance_id, cloned.instance_id);
    assert_eq!(entry.tenant_id, cloned.tenant_id);
    assert_eq!(entry.image_id, cloned.image_id);
    assert_eq!(entry.checkpoint_id, cloned.checkpoint_id);
    assert_eq!(entry.wake_at, cloned.wake_at);
}

// ============================================================================
// Database Wake Operations Tests
// ============================================================================

#[tokio::test]
async fn test_schedule_wake() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let instance_id = Uuid::new_v4().to_string();
    let tenant_id = "test-tenant";
    let image_id = create_test_image(&pool, tenant_id).await;
    create_test_instance(&pool, &instance_id, tenant_id, &image_id).await;

    let wake_at = Utc::now() + ChronoDuration::hours(1);

    db::schedule_wake(&pool, &instance_id, "checkpoint-1", wake_at)
        .await
        .expect("Failed to schedule wake");

    // Verify it was inserted
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT instance_id, checkpoint_id FROM wake_queue WHERE instance_id = $1")
            .bind(&instance_id)
            .fetch_optional(&pool)
            .await
            .unwrap();

    assert!(row.is_some());
    let (id, cp) = row.unwrap();
    assert_eq!(id, instance_id);
    assert_eq!(cp, "checkpoint-1");

    cleanup(&pool, &instance_id).await;
    cleanup_image(&pool, &image_id).await;
}

#[tokio::test]
async fn test_schedule_wake_upsert() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let instance_id = Uuid::new_v4().to_string();
    let tenant_id = "test-tenant";
    let image_id = create_test_image(&pool, tenant_id).await;
    create_test_instance(&pool, &instance_id, tenant_id, &image_id).await;

    let wake_at1 = Utc::now() + ChronoDuration::hours(1);
    let wake_at2 = Utc::now() + ChronoDuration::hours(2);

    // Schedule first wake
    db::schedule_wake(&pool, &instance_id, "cp-1", wake_at1)
        .await
        .unwrap();

    // Schedule second wake (should update)
    db::schedule_wake(&pool, &instance_id, "cp-2", wake_at2)
        .await
        .unwrap();

    // Should only have one entry with updated values
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM wake_queue WHERE instance_id = $1")
        .bind(&instance_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count.0, 1);

    let row: (String,) =
        sqlx::query_as("SELECT checkpoint_id FROM wake_queue WHERE instance_id = $1")
            .bind(&instance_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(row.0, "cp-2");

    cleanup(&pool, &instance_id).await;
    cleanup_image(&pool, &image_id).await;
}

#[tokio::test]
async fn test_get_pending_wakes_ready() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    // Use a unique tenant to avoid conflicts with parallel tests
    let tenant_id = format!("test-tenant-ready-{}", Uuid::new_v4());
    let image_id = create_test_image(&pool, &tenant_id).await;

    // Create instances with past wake times (ready to wake)
    let instance1 = Uuid::new_v4().to_string();
    let instance2 = Uuid::new_v4().to_string();
    create_test_instance(&pool, &instance1, &tenant_id, &image_id).await;
    create_test_instance(&pool, &instance2, &tenant_id, &image_id).await;

    let past_time = Utc::now() - ChronoDuration::minutes(5);
    db::schedule_wake(&pool, &instance1, "cp-1", past_time)
        .await
        .unwrap();
    db::schedule_wake(&pool, &instance2, "cp-2", past_time)
        .await
        .unwrap();

    // Get pending wakes and filter to our instances
    let wakes = db::get_pending_wakes(&pool, 100).await.unwrap();
    let our_wakes: Vec<_> = wakes
        .iter()
        .filter(|w| w.instance_id == instance1 || w.instance_id == instance2)
        .collect();
    assert_eq!(our_wakes.len(), 2);

    cleanup(&pool, &instance1).await;
    cleanup(&pool, &instance2).await;
    cleanup_image(&pool, &image_id).await;
}

#[tokio::test]
async fn test_get_pending_wakes_not_ready() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    // Use a unique tenant to avoid conflicts with parallel tests
    let tenant_id = format!("test-tenant-not-ready-{}", Uuid::new_v4());
    let image_id = create_test_image(&pool, &tenant_id).await;

    let instance_id = Uuid::new_v4().to_string();
    create_test_instance(&pool, &instance_id, &tenant_id, &image_id).await;

    // Schedule wake in the future
    let future_time = Utc::now() + ChronoDuration::hours(1);
    db::schedule_wake(&pool, &instance_id, "cp-1", future_time)
        .await
        .unwrap();

    // Should not return this future wake (check our specific instance)
    let wakes = db::get_pending_wakes(&pool, 100).await.unwrap();
    let our_wake = wakes.iter().find(|w| w.instance_id == instance_id);
    assert!(
        our_wake.is_none(),
        "Future wake should not be in pending wakes"
    );

    cleanup(&pool, &instance_id).await;
    cleanup_image(&pool, &image_id).await;
}

#[tokio::test]
async fn test_get_pending_wakes_limit() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    // Use a unique tenant to avoid conflicts with parallel tests
    let tenant_id = format!("test-tenant-limit-{}", Uuid::new_v4());
    let image_id = create_test_image(&pool, &tenant_id).await;

    let mut instance_ids = Vec::new();
    let past_time = Utc::now() - ChronoDuration::minutes(5);

    // Create 5 instances with ready wakes
    for i in 0..5 {
        let instance_id = Uuid::new_v4().to_string();
        create_test_instance(&pool, &instance_id, &tenant_id, &image_id).await;
        db::schedule_wake(&pool, &instance_id, &format!("cp-{}", i), past_time)
            .await
            .unwrap();
        instance_ids.push(instance_id);
    }

    // Limit to 3 - can return at least 3 (may include from other tests)
    let wakes = db::get_pending_wakes(&pool, 3).await.unwrap();
    assert!(wakes.len() <= 3, "Limit should be respected");

    // Get all our wakes by filtering to our instance_ids
    let wakes = db::get_pending_wakes(&pool, 100).await.unwrap();
    let our_wakes: Vec<_> = wakes
        .iter()
        .filter(|w| instance_ids.contains(&w.instance_id))
        .collect();
    assert_eq!(our_wakes.len(), 5, "Should have 5 wakes for our instances");

    for instance_id in instance_ids {
        cleanup(&pool, &instance_id).await;
    }
    cleanup_image(&pool, &image_id).await;
}

#[tokio::test]
async fn test_get_pending_wakes_ordered_by_time() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    // Use a unique tenant to avoid conflicts with parallel tests
    let tenant_id = format!("test-tenant-ordered-{}", Uuid::new_v4());
    let image_id = create_test_image(&pool, &tenant_id).await;

    let instance1 = Uuid::new_v4().to_string();
    let instance2 = Uuid::new_v4().to_string();
    let instance3 = Uuid::new_v4().to_string();
    create_test_instance(&pool, &instance1, &tenant_id, &image_id).await;
    create_test_instance(&pool, &instance2, &tenant_id, &image_id).await;
    create_test_instance(&pool, &instance3, &tenant_id, &image_id).await;

    // Schedule wakes at different times
    let time1 = Utc::now() - ChronoDuration::minutes(30);
    let time2 = Utc::now() - ChronoDuration::minutes(10);
    let time3 = Utc::now() - ChronoDuration::minutes(20);

    db::schedule_wake(&pool, &instance1, "cp-1", time1)
        .await
        .unwrap();
    db::schedule_wake(&pool, &instance2, "cp-2", time2)
        .await
        .unwrap();
    db::schedule_wake(&pool, &instance3, "cp-3", time3)
        .await
        .unwrap();

    // Get all pending wakes and filter to our instances
    let wakes = db::get_pending_wakes(&pool, 100).await.unwrap();
    let our_wakes: Vec<_> = wakes
        .iter()
        .filter(|w| {
            w.instance_id == instance1 || w.instance_id == instance2 || w.instance_id == instance3
        })
        .collect();

    // Should have all 3 and be ordered by wake_at ascending
    assert_eq!(our_wakes.len(), 3);
    assert_eq!(our_wakes[0].checkpoint_id, "cp-1"); // Oldest
    assert_eq!(our_wakes[1].checkpoint_id, "cp-3"); // Middle
    assert_eq!(our_wakes[2].checkpoint_id, "cp-2"); // Newest

    cleanup(&pool, &instance1).await;
    cleanup(&pool, &instance2).await;
    cleanup(&pool, &instance3).await;
    cleanup_image(&pool, &image_id).await;
}

#[tokio::test]
async fn test_remove_wake() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let instance_id = Uuid::new_v4().to_string();
    let tenant_id = "test-tenant";
    let image_id = create_test_image(&pool, tenant_id).await;
    create_test_instance(&pool, &instance_id, tenant_id, &image_id).await;

    let wake_at = Utc::now() - ChronoDuration::minutes(5);
    db::schedule_wake(&pool, &instance_id, "cp-1", wake_at)
        .await
        .unwrap();

    // Verify it exists
    let row: Option<(String,)> =
        sqlx::query_as("SELECT instance_id FROM wake_queue WHERE instance_id = $1")
            .bind(&instance_id)
            .fetch_optional(&pool)
            .await
            .unwrap();
    assert!(row.is_some());

    // Remove it
    db::remove_wake(&pool, &instance_id).await.unwrap();

    // Verify it's gone
    let row: Option<(String,)> =
        sqlx::query_as("SELECT instance_id FROM wake_queue WHERE instance_id = $1")
            .bind(&instance_id)
            .fetch_optional(&pool)
            .await
            .unwrap();
    assert!(row.is_none());

    cleanup(&pool, &instance_id).await;
    cleanup_image(&pool, &image_id).await;
}

#[tokio::test]
async fn test_remove_wake_nonexistent() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    // Should not error when removing nonexistent
    db::remove_wake(&pool, "nonexistent-instance")
        .await
        .expect("Should succeed even for nonexistent");
}

// ============================================================================
// Instance Database Operations Tests
// ============================================================================

#[tokio::test]
async fn test_create_and_get_instance() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let instance_id = Uuid::new_v4().to_string();
    let tenant_id = "test-tenant";
    let image_id = create_test_image(&pool, tenant_id).await;
    let input = serde_json::json!({"key": "value"});

    db::create_instance(&pool, &instance_id, tenant_id, &image_id, Some(&input))
        .await
        .expect("Failed to create instance");

    let instance = db::get_instance(&pool, &instance_id)
        .await
        .expect("Failed to get instance")
        .expect("Instance should exist");

    assert_eq!(instance.instance_id, instance_id);
    assert_eq!(instance.tenant_id, tenant_id);
    assert_eq!(instance.image_id, Some(image_id.clone()));
    assert_eq!(instance.status, "pending");
    assert_eq!(instance.input, Some(input));
    assert!(instance.output.is_none());
    assert!(instance.error.is_none());

    cleanup(&pool, &instance_id).await;
    cleanup_image(&pool, &image_id).await;
}

#[tokio::test]
async fn test_update_instance_status() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let instance_id = Uuid::new_v4().to_string();
    let tenant_id = "test-tenant";
    let image_id = create_test_image(&pool, tenant_id).await;

    db::create_instance(&pool, &instance_id, tenant_id, &image_id, None)
        .await
        .unwrap();

    // Update to running
    db::update_instance_status(&pool, &instance_id, "running", None)
        .await
        .unwrap();

    let instance = db::get_instance(&pool, &instance_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(instance.status, "running");
    assert!(instance.started_at.is_some()); // Should be set when status = running

    // Update to completed
    db::update_instance_status(&pool, &instance_id, "completed", Some("cp-final"))
        .await
        .unwrap();

    let instance = db::get_instance(&pool, &instance_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(instance.status, "completed");
    assert_eq!(instance.checkpoint_id, Some("cp-final".to_string()));
    assert!(instance.finished_at.is_some());

    cleanup(&pool, &instance_id).await;
    cleanup_image(&pool, &image_id).await;
}

#[tokio::test]
async fn test_update_instance_result() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let instance_id = Uuid::new_v4().to_string();
    let tenant_id = "test-tenant";
    let image_id = create_test_image(&pool, tenant_id).await;

    db::create_instance(&pool, &instance_id, tenant_id, &image_id, None)
        .await
        .unwrap();

    let output = serde_json::json!({"result": "success"});

    db::update_instance_result(
        &pool,
        &instance_id,
        "completed",
        Some(&output),
        None,
        Some("cp-done"),
    )
    .await
    .unwrap();

    let instance = db::get_instance(&pool, &instance_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(instance.status, "completed");
    assert_eq!(instance.output, Some(output));
    assert!(instance.error.is_none());
    assert_eq!(instance.checkpoint_id, Some("cp-done".to_string()));

    cleanup(&pool, &instance_id).await;
    cleanup_image(&pool, &image_id).await;
}

#[tokio::test]
async fn test_update_instance_result_with_error() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let instance_id = Uuid::new_v4().to_string();
    let tenant_id = "test-tenant";
    let image_id = create_test_image(&pool, tenant_id).await;

    db::create_instance(&pool, &instance_id, tenant_id, &image_id, None)
        .await
        .unwrap();

    db::update_instance_result(
        &pool,
        &instance_id,
        "failed",
        None,
        Some("Connection refused"),
        None,
    )
    .await
    .unwrap();

    let instance = db::get_instance(&pool, &instance_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(instance.status, "failed");
    assert!(instance.output.is_none());
    assert_eq!(instance.error, Some("Connection refused".to_string()));

    cleanup(&pool, &instance_id).await;
    cleanup_image(&pool, &image_id).await;
}

#[tokio::test]
async fn test_list_instances() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    // Clean up first
    sqlx::query("DELETE FROM instances WHERE tenant_id LIKE 'list-test-%'")
        .execute(&pool)
        .await
        .ok();

    let image_id = create_test_image(&pool, "list-test-tenant-a").await;

    let instance1 = Uuid::new_v4().to_string();
    let instance2 = Uuid::new_v4().to_string();
    let instance3 = Uuid::new_v4().to_string();

    db::create_instance(&pool, &instance1, "list-test-tenant-a", &image_id, None)
        .await
        .unwrap();
    db::create_instance(&pool, &instance2, "list-test-tenant-a", &image_id, None)
        .await
        .unwrap();
    db::create_instance(&pool, &instance3, "list-test-tenant-b", &image_id, None)
        .await
        .unwrap();

    // Update statuses
    db::update_instance_status(&pool, &instance1, "running", None)
        .await
        .unwrap();
    db::update_instance_status(&pool, &instance2, "completed", None)
        .await
        .unwrap();

    // List all for tenant-a
    let options = db::ListInstancesOptions {
        tenant_id: Some("list-test-tenant-a".to_string()),
        limit: 100,
        ..Default::default()
    };
    let instances = db::list_instances(&pool, &options).await.unwrap();
    assert_eq!(instances.len(), 2);

    // List running for tenant-a
    let options = db::ListInstancesOptions {
        tenant_id: Some("list-test-tenant-a".to_string()),
        status: Some("running".to_string()),
        limit: 100,
        ..Default::default()
    };
    let instances = db::list_instances(&pool, &options).await.unwrap();
    assert_eq!(instances.len(), 1);

    // List all with limit
    let options = db::ListInstancesOptions {
        limit: 2,
        ..Default::default()
    };
    let instances = db::list_instances(&pool, &options).await.unwrap();
    assert_eq!(instances.len(), 2);

    // List with offset
    let all_options = db::ListInstancesOptions {
        limit: 100,
        ..Default::default()
    };
    let all = db::list_instances(&pool, &all_options).await.unwrap();
    let offset_options = db::ListInstancesOptions {
        limit: 100,
        offset: 1,
        ..Default::default()
    };
    let with_offset = db::list_instances(&pool, &offset_options).await.unwrap();
    assert_eq!(with_offset.len(), all.len().saturating_sub(1));

    cleanup(&pool, &instance1).await;
    cleanup(&pool, &instance2).await;
    cleanup(&pool, &instance3).await;
    cleanup_image(&pool, &image_id).await;
}

#[tokio::test]
async fn test_health_check() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let result = db::health_check(&pool)
        .await
        .expect("Health check should succeed");
    assert!(result);
}

// ============================================================================
// WakeScheduler Tests
// ============================================================================

#[tokio::test]
async fn test_wake_scheduler_creation() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let runner = Arc::new(MockRunner::new());
    let config = WakeSchedulerConfig::default();

    let scheduler = WakeScheduler::new(pool, runner, config);
    let _shutdown = scheduler.shutdown_handle();
    // Scheduler created successfully
}

#[tokio::test]
async fn test_wake_scheduler_shutdown() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let runner = Arc::new(MockRunner::new());
    let config = WakeSchedulerConfig {
        poll_interval: Duration::from_millis(50),
        ..Default::default()
    };

    let scheduler = WakeScheduler::new(pool, runner, config);
    let shutdown = scheduler.shutdown_handle();

    // Start the scheduler in a task
    let handle = tokio::spawn(async move {
        scheduler.run().await;
    });

    // Give it a moment to start
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Signal shutdown
    shutdown.notify_one();

    // Wait for it to stop (with timeout)
    let result = tokio::time::timeout(Duration::from_secs(2), handle).await;
    assert!(result.is_ok(), "Scheduler should shutdown promptly");
}

// ============================================================================
// Instance Record Tests
// ============================================================================

#[test]
fn test_instance_debug() {
    let instance = Instance {
        instance_id: "inst-123".to_string(),
        tenant_id: "tenant-456".to_string(),
        image_id: Some(Uuid::new_v4().to_string()),
        status: "running".to_string(),
        input: Some(serde_json::json!({"key": "value"})),
        output: None,
        error: None,
        checkpoint_id: Some("cp-1".to_string()),
        created_at: Utc::now(),
        started_at: Some(Utc::now()),
        finished_at: None,
        retry_count: 0,
        max_retries: 3,
    };

    let debug_str = format!("{:?}", instance);
    assert!(debug_str.contains("inst-123"));
    assert!(debug_str.contains("tenant-456"));
    assert!(debug_str.contains("running"));
}

#[test]
fn test_instance_clone() {
    let instance = Instance {
        instance_id: "i1".to_string(),
        tenant_id: "t1".to_string(),
        image_id: Some(Uuid::new_v4().to_string()),
        status: "pending".to_string(),
        input: None,
        output: None,
        error: None,
        checkpoint_id: None,
        created_at: Utc::now(),
        started_at: None,
        finished_at: None,
        retry_count: 0,
        max_retries: 3,
    };

    let cloned = instance.clone();
    assert_eq!(instance.instance_id, cloned.instance_id);
    assert_eq!(instance.tenant_id, cloned.tenant_id);
    assert_eq!(instance.status, cloned.status);
}

// ============================================================================
// Wake Scheduler Container Registration Tests (Issue #2)
// ============================================================================

/// Test that WakeSchedulerConfig includes data_dir for container monitoring.
/// This field is required for the wake scheduler to spawn container monitors.
#[test]
fn test_wake_scheduler_config_has_data_dir() {
    let config = WakeSchedulerConfig::default();

    // data_dir is required for spawn_container_monitor to process output.json
    assert!(
        !config.data_dir.as_os_str().is_empty(),
        "data_dir should have a default value"
    );
    assert_eq!(config.data_dir, PathBuf::from(".data"));
}

/// Test that custom data_dir can be set in WakeSchedulerConfig.
#[test]
fn test_wake_scheduler_config_custom_data_dir() {
    let config = WakeSchedulerConfig {
        poll_interval: Duration::from_secs(10),
        batch_size: 5,
        core_addr: "127.0.0.1:8001".to_string(),
        data_dir: PathBuf::from("/custom/data/dir"),
    };

    assert_eq!(config.data_dir, PathBuf::from("/custom/data/dir"));
}

/// Test that WakeScheduler can be created with a data_dir configuration.
/// The wake scheduler needs data_dir to spawn container monitors.
#[tokio::test]
async fn test_wake_scheduler_with_data_dir() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let runner = Arc::new(MockRunner::new());
    let temp_dir = tempfile::TempDir::new().unwrap();

    let config = WakeSchedulerConfig {
        poll_interval: Duration::from_secs(5),
        batch_size: 10,
        core_addr: "127.0.0.1:8001".to_string(),
        data_dir: temp_dir.path().to_path_buf(),
    };

    // Create scheduler with custom data_dir
    let scheduler = WakeScheduler::new(pool, runner, config);

    // Scheduler should be created successfully with data_dir
    // The data_dir is used in wake_instance() to call spawn_container_monitor()
    let shutdown = scheduler.shutdown_handle();
    shutdown.notify_one();
}
