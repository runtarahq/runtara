// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Tests for wake_scheduler module and related database operations.

mod common;

use chrono::Utc;
use runtara_environment::db::{self, Instance};
use runtara_environment::wake_scheduler::WakeSchedulerConfig;
use sqlx::PgPool;
use std::path::PathBuf;
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

/// Clean up test data
async fn cleanup(pool: &PgPool, instance_id: &str) {
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

    db::create_instance(&pool, &instance_id, tenant_id, &image_id, None)
        .await
        .expect("Failed to create instance");

    let instance = db::get_instance_full(&pool, &instance_id)
        .await
        .expect("Failed to get instance")
        .expect("Instance should exist");

    assert_eq!(instance.instance_id, instance_id);
    assert_eq!(instance.tenant_id, tenant_id);
    assert_eq!(instance.image_id, Some(image_id.clone()));
    assert_eq!(instance.status, "pending");
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
    let output_bytes = serde_json::to_vec(&output).unwrap();

    db::update_instance_result(
        &pool,
        &instance_id,
        "completed",
        Some(&output_bytes),
        None,
        Some("cp-done"),
        None, // stderr
    )
    .await
    .unwrap();

    let instance = db::get_instance(&pool, &instance_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(instance.status, "completed");
    assert_eq!(instance.output, Some(output_bytes));
    assert!(instance.error.is_none());
    assert!(instance.stderr.is_none());
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
        Some("error: could not connect to server"), // stderr
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
    assert_eq!(
        instance.stderr,
        Some("error: could not connect to server".to_string())
    );

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
// Instance Record Tests
// ============================================================================

#[test]
fn test_instance_debug() {
    let instance = Instance {
        instance_id: "inst-123".to_string(),
        tenant_id: "tenant-456".to_string(),
        status: "running".to_string(),
        checkpoint_id: Some("cp-1".to_string()),
        attempt: 1,
        max_attempts: 3,
        created_at: Utc::now(),
        started_at: Some(Utc::now()),
        finished_at: None,
        output: None,
        error: None,
        stderr: None,
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
        status: "pending".to_string(),
        checkpoint_id: None,
        attempt: 0,
        max_attempts: 3,
        created_at: Utc::now(),
        started_at: None,
        finished_at: None,
        output: None,
        error: None,
        stderr: None,
    };

    let cloned = instance.clone();
    assert_eq!(instance.instance_id, cloned.instance_id);
    assert_eq!(instance.tenant_id, cloned.tenant_id);
    assert_eq!(instance.status, cloned.status);
}

// ============================================================================
// Wake Scheduler Config Tests
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
