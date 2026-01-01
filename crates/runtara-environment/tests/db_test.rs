// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Database operations tests for runtara-environment.
//!
//! These tests verify the correctness of database CRUD operations.

mod common;

use uuid::Uuid;

/// Skip test if database URL is not set
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

async fn get_pool() -> Option<sqlx::PgPool> {
    let database_url = std::env::var("TEST_ENVIRONMENT_DATABASE_URL")
        .or_else(|_| std::env::var("RUNTARA_ENVIRONMENT_DATABASE_URL"))
        .ok()?;
    sqlx::PgPool::connect(&database_url).await.ok()
}

/// Create a test image with a unique name
async fn create_test_image(
    pool: &sqlx::PgPool,
    image_id: &str,
    tenant_id: &str,
) -> Result<(), sqlx::Error> {
    let image_name = format!("test-image-{}", image_id);
    sqlx::query(
        "INSERT INTO images (image_id, tenant_id, name, binary_path, runner_type) VALUES ($1, $2, $3, '/test', 'mock')"
    )
    .bind(image_id)
    .bind(tenant_id)
    .bind(&image_name)
    .execute(pool)
    .await?;
    Ok(())
}

// ============================================================================
// Instance Database Tests
// ============================================================================

#[tokio::test]
async fn test_create_and_get_instance() {
    skip_if_no_db!();
    let pool = get_pool().await.expect("Failed to connect to database");

    let instance_id = Uuid::new_v4().to_string();
    let tenant_id = "test-tenant";
    let image_id = Uuid::new_v4().to_string();

    // Create test image first (foreign key constraint)
    create_test_image(&pool, &image_id, tenant_id)
        .await
        .expect("Failed to create test image");

    // Create instance
    runtara_environment::db::create_instance(&pool, &instance_id, tenant_id, &image_id)
        .await
        .expect("Failed to create instance");

    // Get instance (use get_instance_full to also get image_id)
    let instance = runtara_environment::db::get_instance_full(&pool, &instance_id)
        .await
        .expect("Failed to get instance")
        .expect("Instance not found");

    assert_eq!(instance.instance_id, instance_id);
    assert_eq!(instance.tenant_id, tenant_id);
    assert_eq!(instance.image_id, Some(image_id.clone()));
    assert_eq!(instance.status, "pending");

    // Cleanup
    sqlx::query("DELETE FROM instances WHERE instance_id = $1")
        .bind(&instance_id)
        .execute(&pool)
        .await
        .ok();
    sqlx::query("DELETE FROM images WHERE image_id = $1")
        .bind(&image_id)
        .execute(&pool)
        .await
        .ok();
}

#[tokio::test]
async fn test_update_instance_status() {
    skip_if_no_db!();
    let pool = get_pool().await.expect("Failed to connect to database");

    let instance_id = Uuid::new_v4().to_string();
    let tenant_id = "test-tenant";
    let image_id = Uuid::new_v4().to_string();

    // Create test image
    create_test_image(&pool, &image_id, tenant_id)
        .await
        .expect("Failed to create test image");

    // Create instance
    runtara_environment::db::create_instance(&pool, &instance_id, tenant_id, &image_id)
        .await
        .expect("Failed to create instance");

    // Update to running
    runtara_environment::db::update_instance_status(&pool, &instance_id, "running", None)
        .await
        .expect("Failed to update status");

    let instance = runtara_environment::db::get_instance(&pool, &instance_id)
        .await
        .expect("Failed to get instance")
        .expect("Instance not found");

    assert_eq!(instance.status, "running");
    assert!(instance.started_at.is_some());

    // Update to completed with checkpoint
    runtara_environment::db::update_instance_status(
        &pool,
        &instance_id,
        "completed",
        Some("checkpoint-1"),
    )
    .await
    .expect("Failed to update status");

    let instance = runtara_environment::db::get_instance(&pool, &instance_id)
        .await
        .expect("Failed to get instance")
        .expect("Instance not found");

    assert_eq!(instance.status, "completed");
    assert_eq!(instance.checkpoint_id, Some("checkpoint-1".to_string()));
    assert!(instance.finished_at.is_some());

    // Cleanup
    sqlx::query("DELETE FROM instances WHERE instance_id = $1")
        .bind(&instance_id)
        .execute(&pool)
        .await
        .ok();
    sqlx::query("DELETE FROM images WHERE image_id = $1")
        .bind(&image_id)
        .execute(&pool)
        .await
        .ok();
}

#[tokio::test]
async fn test_update_instance_result() {
    skip_if_no_db!();
    let pool = get_pool().await.expect("Failed to connect to database");

    let instance_id = Uuid::new_v4().to_string();
    let tenant_id = "test-tenant";
    let image_id = Uuid::new_v4().to_string();

    // Create test image
    create_test_image(&pool, &image_id, tenant_id)
        .await
        .expect("Failed to create test image");

    // Create instance
    runtara_environment::db::create_instance(&pool, &instance_id, tenant_id, &image_id)
        .await
        .expect("Failed to create instance");

    // Update with success result
    let output = serde_json::json!({"result": "success"});
    let output_bytes = serde_json::to_vec(&output).unwrap();
    runtara_environment::db::update_instance_result(
        &pool,
        &instance_id,
        "completed",
        Some(&output_bytes),
        None,
        None,
    )
    .await
    .expect("Failed to update result");

    let instance = runtara_environment::db::get_instance(&pool, &instance_id)
        .await
        .expect("Failed to get instance")
        .expect("Instance not found");

    assert_eq!(instance.status, "completed");
    assert_eq!(instance.output, Some(output_bytes));
    assert!(instance.error.is_none());

    // Cleanup
    sqlx::query("DELETE FROM instances WHERE instance_id = $1")
        .bind(&instance_id)
        .execute(&pool)
        .await
        .ok();
    sqlx::query("DELETE FROM images WHERE image_id = $1")
        .bind(&image_id)
        .execute(&pool)
        .await
        .ok();
}

#[tokio::test]
async fn test_update_instance_result_with_error() {
    skip_if_no_db!();
    let pool = get_pool().await.expect("Failed to connect to database");

    let instance_id = Uuid::new_v4().to_string();
    let tenant_id = "test-tenant";
    let image_id = Uuid::new_v4().to_string();

    // Create test image
    create_test_image(&pool, &image_id, tenant_id)
        .await
        .expect("Failed to create test image");

    // Create instance
    runtara_environment::db::create_instance(&pool, &instance_id, tenant_id, &image_id)
        .await
        .expect("Failed to create instance");

    // Update with error result
    runtara_environment::db::update_instance_result(
        &pool,
        &instance_id,
        "failed",
        None,
        Some("Something went wrong"),
        None,
    )
    .await
    .expect("Failed to update result");

    let instance = runtara_environment::db::get_instance(&pool, &instance_id)
        .await
        .expect("Failed to get instance")
        .expect("Instance not found");

    assert_eq!(instance.status, "failed");
    assert!(instance.output.is_none());
    assert_eq!(instance.error, Some("Something went wrong".to_string()));

    // Cleanup
    sqlx::query("DELETE FROM instances WHERE instance_id = $1")
        .bind(&instance_id)
        .execute(&pool)
        .await
        .ok();
    sqlx::query("DELETE FROM images WHERE image_id = $1")
        .bind(&image_id)
        .execute(&pool)
        .await
        .ok();
}

#[tokio::test]
async fn test_list_instances() {
    skip_if_no_db!();
    let pool = get_pool().await.expect("Failed to connect to database");

    let tenant_id = "test-tenant-list";
    let image_id = Uuid::new_v4().to_string();

    // Create test image
    create_test_image(&pool, &image_id, tenant_id)
        .await
        .expect("Failed to create test image");

    // Create multiple instances
    let ids: Vec<_> = (0..3).map(|_| Uuid::new_v4().to_string()).collect();
    for id in &ids {
        runtara_environment::db::create_instance(&pool, id, tenant_id, &image_id)
            .await
            .expect("Failed to create instance");
    }

    // Mark one as completed
    runtara_environment::db::update_instance_status(&pool, &ids[0], "completed", None)
        .await
        .expect("Failed to update status");

    // List all
    let options = runtara_environment::db::ListInstancesOptions {
        tenant_id: Some(tenant_id.to_string()),
        limit: 100,
        ..Default::default()
    };
    let instances = runtara_environment::db::list_instances(&pool, &options)
        .await
        .expect("Failed to list instances");

    assert_eq!(instances.len(), 3);

    // List by status
    let options = runtara_environment::db::ListInstancesOptions {
        tenant_id: Some(tenant_id.to_string()),
        status: Some("completed".to_string()),
        limit: 100,
        ..Default::default()
    };
    let completed = runtara_environment::db::list_instances(&pool, &options)
        .await
        .expect("Failed to list instances");

    assert_eq!(completed.len(), 1);
    assert_eq!(completed[0].instance_id, ids[0]);

    // Cleanup
    for id in &ids {
        sqlx::query("DELETE FROM instances WHERE instance_id = $1")
            .bind(id)
            .execute(&pool)
            .await
            .ok();
    }
    sqlx::query("DELETE FROM images WHERE image_id = $1")
        .bind(&image_id)
        .execute(&pool)
        .await
        .ok();
}

// ============================================================================
// Health Check Test
// ============================================================================

#[tokio::test]
async fn test_health_check() {
    skip_if_no_db!();
    let pool = get_pool().await.expect("Failed to connect to database");

    let healthy = runtara_environment::db::health_check(&pool)
        .await
        .expect("Health check failed");

    assert!(healthy);
}
