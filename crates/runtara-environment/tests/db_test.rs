// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Database operations tests for runtara-environment.
//!
//! These tests verify the correctness of database CRUD operations.

mod common;

use runtara_core::persistence::{Persistence, PostgresPersistence};
use runtara_environment::db;
use sqlx::PgPool;
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

/// Helper to create a test instance using the Persistence trait.
/// This replaces the old `db::create_instance` function that was removed.
async fn create_test_instance(pool: &PgPool, instance_id: &str, tenant_id: &str, image_id: &str) {
    let persistence = PostgresPersistence::new(pool.clone());
    persistence
        .register_instance(instance_id, tenant_id)
        .await
        .expect("Failed to register instance");
    db::associate_instance_image(pool, instance_id, image_id, tenant_id, None)
        .await
        .expect("Failed to associate instance image");
}

/// Helper to create a test instance with env vars using the Persistence trait.
async fn create_test_instance_with_env(
    pool: &PgPool,
    instance_id: &str,
    tenant_id: &str,
    image_id: &str,
    env: Option<&std::collections::HashMap<String, String>>,
) {
    let persistence = PostgresPersistence::new(pool.clone());
    persistence
        .register_instance(instance_id, tenant_id)
        .await
        .expect("Failed to register instance");
    db::associate_instance_image(pool, instance_id, image_id, tenant_id, env)
        .await
        .expect("Failed to associate instance image");
}

/// Helper to update instance status using the Persistence trait.
/// This replaces the old `db::update_instance_status` function that was removed.
async fn update_test_instance_status(
    pool: &PgPool,
    instance_id: &str,
    status: &str,
    checkpoint_id: Option<&str>,
) {
    let persistence = PostgresPersistence::new(pool.clone());
    persistence
        .update_instance_status(instance_id, status, None)
        .await
        .expect("Failed to update instance status");
    if let Some(cp_id) = checkpoint_id {
        persistence
            .update_instance_checkpoint(instance_id, cp_id)
            .await
            .expect("Failed to update instance checkpoint");
    }
}

/// Helper to update instance result using the Persistence trait.
/// This replaces the old `db::update_instance_result` function that was removed.
async fn update_test_instance_result(
    pool: &PgPool,
    instance_id: &str,
    status: &str,
    output: Option<&[u8]>,
    error: Option<&str>,
    checkpoint_id: Option<&str>,
    stderr: Option<&str>,
) {
    let persistence = PostgresPersistence::new(pool.clone());
    persistence
        .complete_instance_extended(instance_id, status, output, error, stderr, checkpoint_id)
        .await
        .expect("Failed to update instance result");
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
    create_test_instance(&pool, &instance_id, tenant_id, &image_id).await;

    // Get instance (use get_instance_full to also get image_id)
    let instance = db::get_instance_full(&pool, &instance_id)
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
    create_test_instance(&pool, &instance_id, tenant_id, &image_id).await;

    // Update to running
    update_test_instance_status(&pool, &instance_id, "running", None).await;

    let instance = db::get_instance(&pool, &instance_id)
        .await
        .expect("Failed to get instance")
        .expect("Instance not found");

    assert_eq!(instance.status, "running");
    assert!(instance.started_at.is_some());

    // Update to completed with checkpoint
    update_test_instance_status(&pool, &instance_id, "completed", Some("checkpoint-1")).await;

    let instance = db::get_instance(&pool, &instance_id)
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
    create_test_instance(&pool, &instance_id, tenant_id, &image_id).await;

    // Update with success result
    let output = serde_json::json!({"result": "success"});
    let output_bytes = serde_json::to_vec(&output).unwrap();
    update_test_instance_result(
        &pool,
        &instance_id,
        "completed",
        Some(&output_bytes),
        None,
        None,
        None, // stderr
    )
    .await;

    let instance = db::get_instance(&pool, &instance_id)
        .await
        .expect("Failed to get instance")
        .expect("Instance not found");

    assert_eq!(instance.status, "completed");
    assert_eq!(instance.output, Some(output_bytes));
    assert!(instance.error.is_none());
    assert!(instance.stderr.is_none());

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
    create_test_instance(&pool, &instance_id, tenant_id, &image_id).await;

    // Update with error result (include stderr for debugging)
    update_test_instance_result(
        &pool,
        &instance_id,
        "failed",
        None,
        Some("Something went wrong"),
        None,
        Some("thread 'main' panicked at 'assertion failed'"), // stderr
    )
    .await;

    let instance = db::get_instance(&pool, &instance_id)
        .await
        .expect("Failed to get instance")
        .expect("Instance not found");

    assert_eq!(instance.status, "failed");
    assert!(instance.output.is_none());
    assert_eq!(instance.error, Some("Something went wrong".to_string()));
    assert_eq!(
        instance.stderr,
        Some("thread 'main' panicked at 'assertion failed'".to_string())
    );

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
        create_test_instance(&pool, id, tenant_id, &image_id).await;
    }

    // Mark one as completed
    update_test_instance_status(&pool, &ids[0], "completed", None).await;

    // List all
    let options = db::ListInstancesOptions {
        tenant_id: Some(tenant_id.to_string()),
        limit: 100,
        ..Default::default()
    };
    let instances = db::list_instances(&pool, &options)
        .await
        .expect("Failed to list instances");

    assert_eq!(instances.len(), 3);

    // List by status
    let options = db::ListInstancesOptions {
        tenant_id: Some(tenant_id.to_string()),
        status: Some("completed".to_string()),
        limit: 100,
        ..Default::default()
    };
    let completed = db::list_instances(&pool, &options)
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

    let healthy = db::health_check(&pool).await.expect("Health check failed");

    assert!(healthy);
}

// ============================================================================
// Environment Variable Persistence Tests
// ============================================================================

#[tokio::test]
async fn test_create_instance_with_env() {
    skip_if_no_db!();
    let pool = get_pool().await.expect("Failed to connect to database");

    let instance_id = Uuid::new_v4().to_string();
    let tenant_id = "test-tenant-env";
    let image_id = Uuid::new_v4().to_string();

    // Create test image first
    create_test_image(&pool, &image_id, tenant_id)
        .await
        .expect("Failed to create test image");

    // Create instance with custom env vars
    let mut env = std::collections::HashMap::new();
    env.insert("API_URL".to_string(), "https://api.example.com".to_string());
    env.insert("DEBUG".to_string(), "true".to_string());

    create_test_instance_with_env(&pool, &instance_id, tenant_id, &image_id, Some(&env)).await;

    // Retrieve and verify env vars
    let result = db::get_instance_image_with_env(&pool, &instance_id)
        .await
        .expect("Failed to get instance env");

    let (retrieved_image_id, retrieved_env) = result.expect("Instance not found");

    assert_eq!(retrieved_image_id, image_id);
    assert_eq!(retrieved_env.len(), 2);
    assert_eq!(
        retrieved_env.get("API_URL").unwrap(),
        "https://api.example.com"
    );
    assert_eq!(retrieved_env.get("DEBUG").unwrap(), "true");

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
async fn test_create_instance_without_env() {
    skip_if_no_db!();
    let pool = get_pool().await.expect("Failed to connect to database");

    let instance_id = Uuid::new_v4().to_string();
    let tenant_id = "test-tenant-no-env";
    let image_id = Uuid::new_v4().to_string();

    // Create test image first
    create_test_image(&pool, &image_id, tenant_id)
        .await
        .expect("Failed to create test image");

    // Create instance without env vars
    create_test_instance(&pool, &instance_id, tenant_id, &image_id).await;

    // Retrieve and verify empty env
    let result = db::get_instance_image_with_env(&pool, &instance_id)
        .await
        .expect("Failed to get instance env");

    let (retrieved_image_id, retrieved_env) = result.expect("Instance not found");

    assert_eq!(retrieved_image_id, image_id);
    assert!(
        retrieved_env.is_empty(),
        "Expected empty env, got {:?}",
        retrieved_env
    );

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
async fn test_get_instance_image_with_env_not_found() {
    skip_if_no_db!();
    let pool = get_pool().await.expect("Failed to connect to database");

    let result = db::get_instance_image_with_env(&pool, "nonexistent-instance")
        .await
        .expect("Query should succeed");

    assert!(result.is_none(), "Expected None for nonexistent instance");
}
