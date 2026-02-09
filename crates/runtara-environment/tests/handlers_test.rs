// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Tests for environment handlers module.

mod common;

use chrono::Utc;
use runtara_core::persistence::{Persistence, PostgresPersistence};
use runtara_environment::db;
use runtara_environment::handlers::{
    EnvironmentHandlerState, GetCapabilityRequest, RegisterImageRequest, ResumeInstanceRequest,
    StartInstanceRequest, StopInstanceRequest, TestCapabilityRequest, handle_get_capability,
    handle_health_check, handle_list_agents, handle_register_image, handle_resume_instance,
    handle_start_instance, handle_stop_instance, handle_test_capability, spawn_container_monitor,
};
use runtara_environment::image_registry::{ImageRegistry, RunnerType};
use runtara_environment::runner::MockRunner;
use runtara_environment::runner::{LaunchOptions, Runner, RunnerHandle};
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

/// Create test handler state
fn create_test_state(pool: PgPool, data_dir: PathBuf) -> EnvironmentHandlerState {
    let runner = Arc::new(MockRunner::new());
    let persistence = Arc::new(PostgresPersistence::new(pool.clone()));
    EnvironmentHandlerState::new(
        pool,
        persistence,
        runner,
        "127.0.0.1:8001".to_string(),
        data_dir,
    )
}

/// Clean up test data
async fn cleanup(pool: &PgPool, instance_id: Option<&str>, image_id: Option<&str>) {
    if let Some(inst_id) = instance_id {
        sqlx::query("DELETE FROM container_registry WHERE instance_id = $1")
            .bind(inst_id)
            .execute(pool)
            .await
            .ok();
        sqlx::query("DELETE FROM container_status WHERE instance_id = $1")
            .bind(inst_id)
            .execute(pool)
            .await
            .ok();
        sqlx::query("DELETE FROM container_cancellations WHERE instance_id = $1")
            .bind(inst_id)
            .execute(pool)
            .await
            .ok();
        sqlx::query("DELETE FROM container_heartbeats WHERE instance_id = $1")
            .bind(inst_id)
            .execute(pool)
            .await
            .ok();
        sqlx::query("DELETE FROM wake_queue WHERE instance_id = $1")
            .bind(inst_id)
            .execute(pool)
            .await
            .ok();
        sqlx::query("DELETE FROM instances WHERE instance_id = $1")
            .bind(inst_id)
            .execute(pool)
            .await
            .ok();
    }
    if let Some(img_id) = image_id {
        sqlx::query("DELETE FROM images WHERE image_id = $1")
            .bind(img_id)
            .execute(pool)
            .await
            .ok();
    }
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

// ============================================================================
// EnvironmentHandlerState Tests
// ============================================================================

#[tokio::test]
async fn test_handler_state_creation() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let temp_dir = tempfile::TempDir::new().unwrap();
    let state = create_test_state(pool, temp_dir.path().to_path_buf());

    assert!(!state.version.is_empty());
    assert!(state.uptime_ms() >= 0);
    assert_eq!(state.core_addr, "127.0.0.1:8001");
}

#[tokio::test]
async fn test_handler_state_uptime() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let temp_dir = tempfile::TempDir::new().unwrap();
    let state = create_test_state(pool, temp_dir.path().to_path_buf());

    let uptime1 = state.uptime_ms();
    tokio::time::sleep(Duration::from_millis(10)).await;
    let uptime2 = state.uptime_ms();

    assert!(uptime2 >= uptime1);
}

// ============================================================================
// Health Check Tests
// ============================================================================

#[tokio::test]
async fn test_health_check_handler() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let temp_dir = tempfile::TempDir::new().unwrap();
    let state = create_test_state(pool, temp_dir.path().to_path_buf());

    let response = handle_health_check(&state)
        .await
        .expect("Health check should succeed");

    assert!(response.healthy);
    assert!(!response.version.is_empty());
    assert!(response.uptime_ms >= 0);
}

// ============================================================================
// Register Image Tests
// ============================================================================

#[tokio::test]
async fn test_register_image_success() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let temp_dir = tempfile::TempDir::new().unwrap();
    let state = create_test_state(pool.clone(), temp_dir.path().to_path_buf());

    let request = RegisterImageRequest {
        tenant_id: "test-tenant".to_string(),
        name: "test-image".to_string(),
        description: Some("Test image description".to_string()),
        binary: vec![0x7f, 0x45, 0x4c, 0x46], // ELF magic bytes
        runner_type: RunnerType::Native,
        metadata: Some(serde_json::json!({"key": "value"})),
    };

    let response = handle_register_image(&state, request)
        .await
        .expect("Register should succeed");

    assert!(response.success, "Error: {:?}", response.error);
    assert!(!response.image_id.is_empty());

    // Verify image was created
    let image_registry = ImageRegistry::new(pool.clone());
    let image = image_registry
        .get(&response.image_id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(image.tenant_id, "test-tenant");
    assert_eq!(image.name, "test-image");

    cleanup(&pool, None, Some(&response.image_id)).await;
}

#[tokio::test]
async fn test_register_image_empty_tenant_id() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let temp_dir = tempfile::TempDir::new().unwrap();
    let state = create_test_state(pool, temp_dir.path().to_path_buf());

    let request = RegisterImageRequest {
        tenant_id: String::new(), // Empty
        name: "test-image".to_string(),
        description: None,
        binary: vec![1, 2, 3],
        runner_type: RunnerType::Native,
        metadata: None,
    };

    let response = handle_register_image(&state, request).await.unwrap();

    assert!(!response.success);
    assert!(response.error.as_ref().unwrap().contains("tenant_id"));
}

#[tokio::test]
async fn test_register_image_empty_name() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let temp_dir = tempfile::TempDir::new().unwrap();
    let state = create_test_state(pool, temp_dir.path().to_path_buf());

    let request = RegisterImageRequest {
        tenant_id: "test-tenant".to_string(),
        name: String::new(), // Empty
        description: None,
        binary: vec![1, 2, 3],
        runner_type: RunnerType::Native,
        metadata: None,
    };

    let response = handle_register_image(&state, request).await.unwrap();

    assert!(!response.success);
    assert!(response.error.as_ref().unwrap().contains("name"));
}

#[tokio::test]
async fn test_register_image_empty_binary() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let temp_dir = tempfile::TempDir::new().unwrap();
    let state = create_test_state(pool, temp_dir.path().to_path_buf());

    let request = RegisterImageRequest {
        tenant_id: "test-tenant".to_string(),
        name: "test-image".to_string(),
        description: None,
        binary: vec![], // Empty
        runner_type: RunnerType::Native,
        metadata: None,
    };

    let response = handle_register_image(&state, request).await.unwrap();

    assert!(!response.success);
    assert!(response.error.as_ref().unwrap().contains("binary"));
}

// ============================================================================
// Start Instance Tests
// ============================================================================

#[tokio::test]
async fn test_start_instance_success() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let temp_dir = tempfile::TempDir::new().unwrap();
    let state = create_test_state(pool.clone(), temp_dir.path().to_path_buf());

    // First register an image
    let image_id = Uuid::new_v4().to_string();
    let image_name = format!("test-image-{}", image_id);
    sqlx::query(
        r#"
        INSERT INTO images (image_id, tenant_id, name, description, binary_path, bundle_path, runner_type)
        VALUES ($1, 'test-tenant', $2, 'desc', '/bin/true', '/tmp/test-bundle', 'mock')
        "#,
    )
    .bind(&image_id)
    .bind(&image_name)
    .execute(&pool)
    .await
    .unwrap();

    let request = StartInstanceRequest {
        image_id: image_id.clone(),
        tenant_id: "test-tenant".to_string(),
        instance_id: None,
        input: Some(serde_json::json!({"key": "value"})),
        timeout_seconds: Some(60),
        env: std::collections::HashMap::new(),
    };

    let response = handle_start_instance(&state, request)
        .await
        .expect("Start should succeed");

    assert!(response.success, "Error: {:?}", response.error);
    assert!(!response.instance_id.is_empty());

    // Verify instance was created in DB
    let instance = db::get_instance(&pool, &response.instance_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(instance.tenant_id, "test-tenant");

    cleanup(&pool, Some(&response.instance_id), Some(&image_id)).await;
}

#[tokio::test]
async fn test_start_instance_with_custom_id() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let temp_dir = tempfile::TempDir::new().unwrap();
    let state = create_test_state(pool.clone(), temp_dir.path().to_path_buf());

    // First register an image
    let image_id = Uuid::new_v4().to_string();
    let image_name = format!("test-image-{}", image_id);
    sqlx::query(
        r#"
        INSERT INTO images (image_id, tenant_id, name, description, binary_path, bundle_path, runner_type)
        VALUES ($1, 'test-tenant', $2, 'desc', '/bin/true', '/tmp/test-bundle', 'mock')
        "#,
    )
    .bind(&image_id)
    .bind(&image_name)
    .execute(&pool)
    .await
    .unwrap();

    let custom_instance_id = format!("custom-{}", Uuid::new_v4());

    let request = StartInstanceRequest {
        image_id: image_id.clone(),
        tenant_id: "test-tenant".to_string(),
        instance_id: Some(custom_instance_id.clone()),
        input: None,
        timeout_seconds: None,
        env: std::collections::HashMap::new(),
    };

    let response = handle_start_instance(&state, request).await.unwrap();

    assert!(response.success, "Error: {:?}", response.error);
    assert_eq!(response.instance_id, custom_instance_id);

    cleanup(&pool, Some(&response.instance_id), Some(&image_id)).await;
}

#[tokio::test]
async fn test_start_instance_empty_image_id() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let temp_dir = tempfile::TempDir::new().unwrap();
    let state = create_test_state(pool, temp_dir.path().to_path_buf());

    let request = StartInstanceRequest {
        image_id: "".to_string(),
        tenant_id: "test-tenant".to_string(),
        instance_id: None,
        input: None,
        timeout_seconds: None,
        env: std::collections::HashMap::new(),
    };

    let response = handle_start_instance(&state, request).await.unwrap();

    assert!(!response.success);
    assert!(
        response
            .error
            .as_ref()
            .unwrap()
            .contains("image_id is required")
    );
}

#[tokio::test]
async fn test_start_instance_image_not_found() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let temp_dir = tempfile::TempDir::new().unwrap();
    let state = create_test_state(pool, temp_dir.path().to_path_buf());

    let request = StartInstanceRequest {
        image_id: "nonexistent-image-id".to_string(),
        tenant_id: "test-tenant".to_string(),
        instance_id: None,
        input: None,
        timeout_seconds: None,
        env: std::collections::HashMap::new(),
    };

    let response = handle_start_instance(&state, request).await.unwrap();

    assert!(!response.success);
    assert!(response.error.as_ref().unwrap().contains("not found"));
}

// ============================================================================
// Stop Instance Tests
// ============================================================================

#[tokio::test]
async fn test_stop_instance_not_found() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let temp_dir = tempfile::TempDir::new().unwrap();
    let state = create_test_state(pool, temp_dir.path().to_path_buf());

    let request = StopInstanceRequest {
        instance_id: "nonexistent-instance".to_string(),
        reason: "test".to_string(),
        grace_period_seconds: 10,
    };

    let response = handle_stop_instance(&state, request).await.unwrap();

    assert!(!response.success);
    assert!(response.error.as_ref().unwrap().contains("not found"));
}

#[tokio::test]
async fn test_stop_instance_with_registered_container() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let temp_dir = tempfile::TempDir::new().unwrap();
    let state = create_test_state(pool.clone(), temp_dir.path().to_path_buf());

    let instance_id = Uuid::new_v4().to_string();

    // Create an image and instance
    let image_id = Uuid::new_v4().to_string();
    let image_name = format!("test-image-{}", image_id);
    sqlx::query(
        r#"
        INSERT INTO images (image_id, tenant_id, name, description, binary_path, bundle_path, runner_type)
        VALUES ($1, 'test-tenant', $2, 'desc', '/bin/true', '/tmp/test-bundle', 'mock')
        "#,
    )
    .bind(&image_id)
    .bind(&image_name)
    .execute(&pool)
    .await
    .unwrap();

    create_test_instance(&pool, &instance_id, "test-tenant", &image_id).await;

    // Register in container registry
    let container_registry =
        runtara_environment::container_registry::ContainerRegistry::new(pool.clone());
    let container_info = runtara_environment::container_registry::ContainerInfo {
        container_id: format!("container-{}", instance_id),
        instance_id: instance_id.clone(),
        tenant_id: "test-tenant".to_string(),
        binary_path: "/bin/true".to_string(),
        bundle_path: None,
        started_at: Utc::now(),
        pid: None,
        timeout_seconds: Some(300),
        process_killed: false,
    };
    container_registry.register(&container_info).await.unwrap();

    let request = StopInstanceRequest {
        instance_id: instance_id.clone(),
        reason: "Testing stop".to_string(),
        grace_period_seconds: 5,
    };

    let response = handle_stop_instance(&state, request).await.unwrap();

    assert!(response.success, "Error: {:?}", response.error);

    // Verify instance status was updated
    let instance = db::get_instance(&pool, &instance_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(instance.status, "cancelled");

    cleanup(&pool, Some(&instance_id), Some(&image_id)).await;
}

// ============================================================================
// Resume Instance Tests
// ============================================================================

#[tokio::test]
async fn test_resume_instance_not_found() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let temp_dir = tempfile::TempDir::new().unwrap();
    let state = create_test_state(pool, temp_dir.path().to_path_buf());

    let request = ResumeInstanceRequest {
        instance_id: "nonexistent-instance".to_string(),
    };

    let response = handle_resume_instance(&state, request).await.unwrap();

    assert!(!response.success);
    assert!(response.error.as_ref().unwrap().contains("not found"));
}

#[tokio::test]
async fn test_resume_instance_wrong_status() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let temp_dir = tempfile::TempDir::new().unwrap();
    let state = create_test_state(pool.clone(), temp_dir.path().to_path_buf());

    let instance_id = Uuid::new_v4().to_string();
    let image_id = Uuid::new_v4().to_string();

    // Create image and instance in "running" state
    let image_name = format!("test-image-{}", image_id);
    sqlx::query(
        r#"
        INSERT INTO images (image_id, tenant_id, name, description, binary_path, bundle_path, runner_type)
        VALUES ($1, 'test-tenant', $2, 'desc', '/bin/true', '/tmp/test-bundle', 'mock')
        "#,
    )
    .bind(&image_id)
    .bind(&image_name)
    .execute(&pool)
    .await
    .unwrap();

    create_test_instance(&pool, &instance_id, "test-tenant", &image_id).await;
    update_test_instance_status(&pool, &instance_id, "running", None).await;

    let request = ResumeInstanceRequest {
        instance_id: instance_id.clone(),
    };

    let response = handle_resume_instance(&state, request).await.unwrap();

    assert!(!response.success);
    assert!(
        response
            .error
            .as_ref()
            .unwrap()
            .contains("must be suspended")
    );

    cleanup(&pool, Some(&instance_id), Some(&image_id)).await;
}

#[tokio::test]
async fn test_resume_instance_no_checkpoint() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let temp_dir = tempfile::TempDir::new().unwrap();
    let state = create_test_state(pool.clone(), temp_dir.path().to_path_buf());

    let instance_id = Uuid::new_v4().to_string();
    let image_id = Uuid::new_v4().to_string();

    // Create image and instance in "suspended" state but without checkpoint
    let image_name = format!("test-image-{}", image_id);
    sqlx::query(
        r#"
        INSERT INTO images (image_id, tenant_id, name, description, binary_path, bundle_path, runner_type)
        VALUES ($1, 'test-tenant', $2, 'desc', '/bin/true', '/tmp/test-bundle', 'mock')
        "#,
    )
    .bind(&image_id)
    .bind(&image_name)
    .execute(&pool)
    .await
    .unwrap();

    create_test_instance(&pool, &instance_id, "test-tenant", &image_id).await;
    update_test_instance_status(&pool, &instance_id, "suspended", None).await;

    let request = ResumeInstanceRequest {
        instance_id: instance_id.clone(),
    };

    let response = handle_resume_instance(&state, request).await.unwrap();

    assert!(!response.success);
    assert!(response.error.as_ref().unwrap().contains("no checkpoint"));

    cleanup(&pool, Some(&instance_id), Some(&image_id)).await;
}

#[tokio::test]
async fn test_resume_instance_success() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let temp_dir = tempfile::TempDir::new().unwrap();
    let state = create_test_state(pool.clone(), temp_dir.path().to_path_buf());

    let instance_id = Uuid::new_v4().to_string();
    let image_id = Uuid::new_v4().to_string();

    // Create image and instance in proper suspended state with checkpoint
    let image_name = format!("test-image-{}", image_id);
    sqlx::query(
        r#"
        INSERT INTO images (image_id, tenant_id, name, description, binary_path, bundle_path, runner_type)
        VALUES ($1, 'test-tenant', $2, 'desc', '/bin/true', '/tmp/test-bundle', 'mock')
        "#,
    )
    .bind(&image_id)
    .bind(&image_name)
    .execute(&pool)
    .await
    .unwrap();

    create_test_instance(&pool, &instance_id, "test-tenant", &image_id).await;
    update_test_instance_status(&pool, &instance_id, "suspended", Some("checkpoint-123")).await;

    let request = ResumeInstanceRequest {
        instance_id: instance_id.clone(),
    };

    let response = handle_resume_instance(&state, request).await.unwrap();

    assert!(response.success, "Error: {:?}", response.error);

    // Verify instance status was updated to running
    let instance = db::get_instance(&pool, &instance_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(instance.status, "running");

    cleanup(&pool, Some(&instance_id), Some(&image_id)).await;
}

// ============================================================================
// Response Type Tests
// ============================================================================

#[test]
fn test_health_check_response_debug() {
    let response = runtara_environment::handlers::HealthCheckResponse {
        healthy: true,
        version: "1.0.0".to_string(),
        uptime_ms: 12345,
    };
    let debug_str = format!("{:?}", response);
    assert!(debug_str.contains("healthy"));
    assert!(debug_str.contains("1.0.0"));
    assert!(debug_str.contains("12345"));
}

#[test]
fn test_runner_type_values() {
    assert_eq!(RunnerType::Native.to_string(), "native");
    assert_eq!(RunnerType::Oci.to_string(), "oci");
    assert_eq!(RunnerType::Wasm.to_string(), "wasm");
}

// ============================================================================
// Multi-Tenant Isolation Tests (Issue #1)
// ============================================================================

/// Test that a tenant cannot start an instance using another tenant's image.
/// This is a critical security test for multi-tenant isolation.
#[tokio::test]
async fn test_start_instance_tenant_isolation() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let temp_dir = tempfile::TempDir::new().unwrap();
    let state = create_test_state(pool.clone(), temp_dir.path().to_path_buf());

    // Register an image owned by tenant-A
    let image_id = Uuid::new_v4().to_string();
    let image_name = format!("tenant-a-image-{}", image_id);
    sqlx::query(
        r#"
        INSERT INTO images (image_id, tenant_id, name, description, binary_path, bundle_path, runner_type)
        VALUES ($1, 'tenant-A', $2, 'Owned by tenant A', '/bin/true', '/tmp/test-bundle', 'mock')
        "#,
    )
    .bind(&image_id)
    .bind(&image_name)
    .execute(&pool)
    .await
    .unwrap();

    // Attempt to start an instance as tenant-B using tenant-A's image
    let request = StartInstanceRequest {
        image_id: image_id.clone(),
        tenant_id: "tenant-B".to_string(), // Different tenant!
        instance_id: None,
        input: None,
        timeout_seconds: None,
        env: std::collections::HashMap::new(),
    };

    let response = handle_start_instance(&state, request).await.unwrap();

    // Should fail - tenant-B should not be able to use tenant-A's image
    assert!(
        !response.success,
        "Tenant isolation breach: tenant-B should not be able to use tenant-A's image"
    );
    assert!(
        response.error.as_ref().unwrap().contains("not found"),
        "Error should indicate image not found (hiding existence from wrong tenant)"
    );

    cleanup(&pool, None, Some(&image_id)).await;
}

/// Test that a tenant CAN start an instance using their own image.
#[tokio::test]
async fn test_start_instance_same_tenant_allowed() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let temp_dir = tempfile::TempDir::new().unwrap();
    let state = create_test_state(pool.clone(), temp_dir.path().to_path_buf());

    // Register an image owned by tenant-A
    let image_id = Uuid::new_v4().to_string();
    let image_name = format!("tenant-a-image-{}", image_id);
    sqlx::query(
        r#"
        INSERT INTO images (image_id, tenant_id, name, description, binary_path, bundle_path, runner_type)
        VALUES ($1, 'tenant-A', $2, 'Owned by tenant A', '/bin/true', '/tmp/test-bundle', 'mock')
        "#,
    )
    .bind(&image_id)
    .bind(&image_name)
    .execute(&pool)
    .await
    .unwrap();

    // Start an instance as tenant-A using tenant-A's image
    let request = StartInstanceRequest {
        image_id: image_id.clone(),
        tenant_id: "tenant-A".to_string(), // Same tenant
        instance_id: None,
        input: None,
        timeout_seconds: None,
        env: std::collections::HashMap::new(),
    };

    let response = handle_start_instance(&state, request).await.unwrap();

    // Should succeed
    assert!(response.success, "Error: {:?}", response.error);
    assert!(!response.instance_id.is_empty());

    cleanup(&pool, Some(&response.instance_id), Some(&image_id)).await;
}

// ============================================================================
// Agent Testing Handler Tests
// ============================================================================

#[tokio::test]
async fn test_list_agents_returns_valid_json() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let temp_dir = tempfile::TempDir::new().unwrap();
    let state = create_test_state(pool, temp_dir.path().to_path_buf());

    let response = handle_list_agents(&state)
        .await
        .expect("List agents should succeed");

    // Should return valid JSON
    let agents: serde_json::Value =
        serde_json::from_slice(&response.agents_json).expect("Should be valid JSON");

    // Should be an array
    assert!(
        agents.is_array(),
        "Agents response should be an array, got: {:?}",
        agents
    );

    // Note: In the test environment, runtara-workflow-stdlib is not linked,
    // so agents are not registered via the inventory crate. The list may be empty.
    // In production, when the test harness binary runs (which links runtara-workflow-stdlib),
    // agents will be available.

    // If agents are present (e.g., in an integration test with full dependencies),
    // verify they have required fields
    let agents_arr = agents.as_array().unwrap();
    for agent in agents_arr {
        assert!(
            agent.get("id").is_some(),
            "Agent should have 'id' field: {:?}",
            agent
        );
        assert!(
            agent.get("name").is_some(),
            "Agent should have 'name' field: {:?}",
            agent
        );
    }
}

#[tokio::test]
async fn test_get_capability_handler_returns_response() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let temp_dir = tempfile::TempDir::new().unwrap();
    let state = create_test_state(pool, temp_dir.path().to_path_buf());

    // Try to get the utils/random-double capability
    // Note: In unit tests, agents are not registered (runtara-workflow-stdlib not linked),
    // so this will return found=false. In E2E tests with full binary, it would work.
    let request = GetCapabilityRequest {
        agent_id: "utils".to_string(),
        capability_id: "random-double".to_string(),
    };

    let response = handle_get_capability(&state, request)
        .await
        .expect("Get capability should succeed");

    // The handler should return a valid response (even if not found in unit test context)
    // If found, inputs_json should be valid JSON
    if response.found && !response.inputs_json.is_empty() {
        let _inputs: serde_json::Value = serde_json::from_slice(&response.inputs_json)
            .expect("inputs_json should be valid JSON");
    }
}

#[tokio::test]
async fn test_get_capability_not_found() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let temp_dir = tempfile::TempDir::new().unwrap();
    let state = create_test_state(pool, temp_dir.path().to_path_buf());

    let request = GetCapabilityRequest {
        agent_id: "nonexistent-agent".to_string(),
        capability_id: "nonexistent-capability".to_string(),
    };

    let response = handle_get_capability(&state, request)
        .await
        .expect("Get capability should not error");

    assert!(
        !response.found,
        "Nonexistent capability should not be found"
    );
    assert!(
        response.inputs_json.is_empty(),
        "inputs_json should be empty for not found"
    );
}

#[tokio::test]
async fn test_get_capability_wrong_agent() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let temp_dir = tempfile::TempDir::new().unwrap();
    let state = create_test_state(pool, temp_dir.path().to_path_buf());

    // Use a valid capability ID but with wrong agent
    let request = GetCapabilityRequest {
        agent_id: "http".to_string(),               // Wrong agent
        capability_id: "random-double".to_string(), // This belongs to utils
    };

    let response = handle_get_capability(&state, request)
        .await
        .expect("Get capability should not error");

    assert!(
        !response.found,
        "Capability with wrong agent should not be found"
    );
}

#[tokio::test]
async fn test_test_capability_request_creation() {
    // Unit test for TestCapabilityRequest struct
    let request = TestCapabilityRequest {
        tenant_id: "test-tenant".to_string(),
        agent_id: "utils".to_string(),
        capability_id: "random-double".to_string(),
        input: serde_json::json!({}),
        connection: None,
        timeout_ms: Some(5000),
    };

    assert_eq!(request.tenant_id, "test-tenant");
    assert_eq!(request.agent_id, "utils");
    assert_eq!(request.capability_id, "random-double");
    assert_eq!(request.timeout_ms, Some(5000));
    assert!(request.connection.is_none());
}

#[tokio::test]
async fn test_test_capability_with_connection() {
    // Unit test for TestCapabilityRequest with connection
    let connection = serde_json::json!({
        "integration_id": "bearer",
        "parameters": {
            "base_url": "https://api.example.com",
            "token": "secret-token"
        }
    });

    let request = TestCapabilityRequest {
        tenant_id: "test-tenant".to_string(),
        agent_id: "http".to_string(),
        capability_id: "http-request".to_string(),
        input: serde_json::json!({
            "url": "/api/users",
            "method": "GET"
        }),
        connection: Some(connection.clone()),
        timeout_ms: None,
    };

    assert!(request.connection.is_some());
    assert_eq!(
        request.connection.as_ref().unwrap()["integration_id"],
        "bearer"
    );
}

// Note: Full integration tests for handle_test_capability require OCI runtime
// and compiled test harness binary, so they are best run in E2E tests.
// The following test validates the handler returns appropriate errors when
// test harness is not available.

#[tokio::test]
async fn test_test_capability_no_harness_binary() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    // Create a temp dir that doesn't have the test harness binary
    let temp_dir = tempfile::TempDir::new().unwrap();
    let state = create_test_state(pool, temp_dir.path().to_path_buf());

    let request = TestCapabilityRequest {
        tenant_id: "test-tenant".to_string(),
        agent_id: "utils".to_string(),
        capability_id: "random-double".to_string(),
        input: serde_json::json!({}),
        connection: None,
        timeout_ms: Some(1000),
    };

    let response = handle_test_capability(&state, request)
        .await
        .expect("Test capability should not panic");

    // Without a compiled test harness binary, this should fail gracefully
    assert!(!response.success);
    assert!(response.error.is_some());
    // Error should indicate test harness is not available
    let error = response.error.as_ref().unwrap();
    assert!(
        error.contains("harness") || error.contains("not found") || error.contains("not available"),
        "Error should mention harness issue: {}",
        error
    );
}

// ============================================================================
// Environment Variable Persistence Tests
// ============================================================================

/// Test that env vars passed to start_instance are stored in the database
#[tokio::test]
async fn test_start_instance_stores_env() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let temp_dir = tempfile::TempDir::new().unwrap();
    let state = create_test_state(pool.clone(), temp_dir.path().to_path_buf());

    // Register an image
    let image_id = Uuid::new_v4().to_string();
    let image_name = format!("test-image-env-{}", image_id);
    sqlx::query(
        r#"
        INSERT INTO images (image_id, tenant_id, name, description, binary_path, bundle_path, runner_type)
        VALUES ($1, 'test-tenant', $2, 'desc', '/bin/true', '/tmp/test-bundle', 'mock')
        "#,
    )
    .bind(&image_id)
    .bind(&image_name)
    .execute(&pool)
    .await
    .unwrap();

    // Create env vars
    let mut env = std::collections::HashMap::new();
    env.insert("API_URL".to_string(), "https://api.example.com".to_string());
    env.insert("SECRET_KEY".to_string(), "my-secret".to_string());

    let request = StartInstanceRequest {
        image_id: image_id.clone(),
        tenant_id: "test-tenant".to_string(),
        instance_id: None,
        input: None,
        timeout_seconds: None,
        env,
    };

    let response = handle_start_instance(&state, request).await.unwrap();
    assert!(response.success, "Error: {:?}", response.error);

    // Verify env vars were stored in the database
    let result = db::get_instance_image_with_env(&pool, &response.instance_id)
        .await
        .expect("Failed to get instance env");

    let (retrieved_image_id, retrieved_env) = result.expect("Instance not found");
    assert_eq!(retrieved_image_id, image_id);
    assert_eq!(retrieved_env.len(), 2);
    assert_eq!(
        retrieved_env.get("API_URL").unwrap(),
        "https://api.example.com"
    );
    assert_eq!(retrieved_env.get("SECRET_KEY").unwrap(), "my-secret");

    cleanup(&pool, Some(&response.instance_id), Some(&image_id)).await;
}

/// Test that empty env is handled correctly
#[tokio::test]
async fn test_start_instance_empty_env() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let temp_dir = tempfile::TempDir::new().unwrap();
    let state = create_test_state(pool.clone(), temp_dir.path().to_path_buf());

    // Register an image
    let image_id = Uuid::new_v4().to_string();
    let image_name = format!("test-image-no-env-{}", image_id);
    sqlx::query(
        r#"
        INSERT INTO images (image_id, tenant_id, name, description, binary_path, bundle_path, runner_type)
        VALUES ($1, 'test-tenant', $2, 'desc', '/bin/true', '/tmp/test-bundle', 'mock')
        "#,
    )
    .bind(&image_id)
    .bind(&image_name)
    .execute(&pool)
    .await
    .unwrap();

    let request = StartInstanceRequest {
        image_id: image_id.clone(),
        tenant_id: "test-tenant".to_string(),
        instance_id: None,
        input: None,
        timeout_seconds: None,
        env: std::collections::HashMap::new(), // Empty env
    };

    let response = handle_start_instance(&state, request).await.unwrap();
    assert!(response.success, "Error: {:?}", response.error);

    // Verify empty env is stored correctly (should return empty HashMap)
    let result = db::get_instance_image_with_env(&pool, &response.instance_id)
        .await
        .expect("Failed to get instance env");

    let (_, retrieved_env) = result.expect("Instance not found");
    assert!(
        retrieved_env.is_empty(),
        "Expected empty env, got {:?}",
        retrieved_env
    );

    cleanup(&pool, Some(&response.instance_id), Some(&image_id)).await;
}

// ============================================================================
// spawn_container_monitor Timeout Tests
// ============================================================================

/// Test that spawn_container_monitor enforces execution timeout.
///
/// This test verifies that:
/// 1. When timeout is exceeded, the container is stopped
/// 2. Instance status is updated to "failed"
/// 3. Error message indicates timeout
#[tokio::test]
async fn test_spawn_container_monitor_timeout_enforcement() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let temp_dir = tempfile::tempdir().unwrap();
    let instance_id = Uuid::new_v4().to_string();
    let tenant_id = "test-tenant-timeout";

    // Create a runner that never completes on its own
    let runner = Arc::new(MockRunner::never_completing());
    let persistence = Arc::new(PostgresPersistence::new(pool.clone()));

    // Register the instance first
    persistence
        .register_instance(&instance_id, tenant_id)
        .await
        .expect("Failed to register instance");

    // Update status to running (required for complete_instance_if_running to work)
    persistence
        .update_instance_status(&instance_id, "running", Some(Utc::now()))
        .await
        .expect("Failed to update instance status");

    // Create a handle for the "running" container
    let handle = RunnerHandle {
        handle_id: format!("mock_{}", &instance_id[..8]),
        instance_id: instance_id.clone(),
        tenant_id: tenant_id.to_string(),
        started_at: Utc::now(),
        spawned_pid: None,
    };

    // Register the mock instance in the runner
    runner
        .launch_detached(&LaunchOptions {
            instance_id: instance_id.clone(),
            tenant_id: tenant_id.to_string(),
            bundle_path: PathBuf::from("/test/bundle"),
            input: serde_json::json!({}),
            timeout: Duration::from_millis(100),
            runtara_core_addr: "127.0.0.1:8001".to_string(),
            checkpoint_id: None,
            env: std::collections::HashMap::new(),
        })
        .await
        .expect("Failed to launch detached");

    // Verify runner shows as running
    assert!(
        runner.is_running(&handle).await,
        "Runner should be running initially"
    );

    // Spawn the monitor with a very short timeout (100ms)
    spawn_container_monitor(
        pool.clone(),
        runner.clone(),
        handle.clone(),
        tenant_id.to_string(),
        temp_dir.path().to_path_buf(),
        persistence.clone(),
        Duration::from_millis(100),
        None, // No PID for test
    );

    // Wait for the timeout to trigger (100ms timeout + some buffer for processing)
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Verify the runner was stopped
    assert!(
        !runner.is_running(&handle).await,
        "Runner should be stopped after timeout"
    );

    // Verify instance status was updated to failed
    let instance = persistence
        .get_instance(&instance_id)
        .await
        .expect("Failed to get instance")
        .expect("Instance not found");

    assert_eq!(
        instance.status, "failed",
        "Instance status should be 'failed'"
    );
    assert!(
        instance
            .error
            .as_ref()
            .is_some_and(|e| e.contains("timed out")),
        "Error should mention timeout, got: {:?}",
        instance.error
    );

    // Cleanup
    cleanup(&pool, Some(&instance_id), None).await;
}

/// Test that spawn_container_monitor does NOT timeout when container completes quickly.
///
/// This test verifies that:
/// 1. When container completes before timeout, no timeout error occurs
/// 2. Instance can complete successfully
#[tokio::test]
async fn test_spawn_container_monitor_no_timeout_on_quick_completion() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let temp_dir = tempfile::tempdir().unwrap();
    let instance_id = Uuid::new_v4().to_string();
    let tenant_id = "test-tenant-no-timeout";

    // Create a runner that completes quickly (default 10ms)
    let runner = Arc::new(MockRunner::new());
    let persistence = Arc::new(PostgresPersistence::new(pool.clone()));

    // Register the instance first
    persistence
        .register_instance(&instance_id, tenant_id)
        .await
        .expect("Failed to register instance");

    // Update status to running
    persistence
        .update_instance_status(&instance_id, "running", Some(Utc::now()))
        .await
        .expect("Failed to update instance status");

    // Launch detached (this will auto-complete in 10ms)
    let handle = runner
        .launch_detached(&LaunchOptions {
            instance_id: instance_id.clone(),
            tenant_id: tenant_id.to_string(),
            bundle_path: PathBuf::from("/test/bundle"),
            input: serde_json::json!({}),
            timeout: Duration::from_secs(10), // Long timeout
            runtara_core_addr: "127.0.0.1:8001".to_string(),
            checkpoint_id: None,
            env: std::collections::HashMap::new(),
        })
        .await
        .expect("Failed to launch detached");

    // Spawn the monitor with a long timeout (10 seconds - should never trigger)
    spawn_container_monitor(
        pool.clone(),
        runner.clone(),
        handle.clone(),
        tenant_id.to_string(),
        temp_dir.path().to_path_buf(),
        persistence.clone(),
        Duration::from_secs(10),
        None, // No PID for test
    );

    // Wait for the container to complete (10ms delay + buffer)
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Verify the runner is no longer running (completed naturally)
    assert!(
        !runner.is_running(&handle).await,
        "Runner should have completed"
    );

    // Verify instance status was NOT set to failed due to timeout
    // Note: The monitor doesn't set status to "completed" - that's done by the SDK via Core.
    // It only processes output. So we check that status is NOT "failed" with timeout error.
    let instance = persistence
        .get_instance(&instance_id)
        .await
        .expect("Failed to get instance")
        .expect("Instance not found");

    // The status might still be "running" since we didn't simulate SDK completion,
    // but it should NOT be "failed" with timeout error
    if instance.status == "failed" {
        assert!(
            !instance
                .error
                .as_ref()
                .is_some_and(|e| e.contains("timed out")),
            "Should not have timeout error on quick completion"
        );
    }

    // Cleanup
    cleanup(&pool, Some(&instance_id), None).await;
}

/// Test that spawn_container_monitor timeout respects race conditions.
///
/// This verifies the race condition handling via complete_instance_if_running:
/// if another process (like Core) already marked the instance as completed,
/// the timeout handler should not overwrite it.
#[tokio::test]
async fn test_spawn_container_monitor_timeout_race_condition() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let temp_dir = tempfile::tempdir().unwrap();
    let instance_id = Uuid::new_v4().to_string();
    let tenant_id = "test-tenant-race";

    // Create a runner that never completes
    let runner = Arc::new(MockRunner::never_completing());
    let persistence = Arc::new(PostgresPersistence::new(pool.clone()));

    // Register the instance
    persistence
        .register_instance(&instance_id, tenant_id)
        .await
        .expect("Failed to register instance");

    // Start with running status
    persistence
        .update_instance_status(&instance_id, "running", Some(Utc::now()))
        .await
        .expect("Failed to update instance status");

    let handle = runner
        .launch_detached(&LaunchOptions {
            instance_id: instance_id.clone(),
            tenant_id: tenant_id.to_string(),
            bundle_path: PathBuf::from("/test/bundle"),
            input: serde_json::json!({}),
            timeout: Duration::from_millis(200),
            runtara_core_addr: "127.0.0.1:8001".to_string(),
            checkpoint_id: None,
            env: std::collections::HashMap::new(),
        })
        .await
        .expect("Failed to launch detached");

    // Spawn the monitor with a 200ms timeout
    spawn_container_monitor(
        pool.clone(),
        runner.clone(),
        handle.clone(),
        tenant_id.to_string(),
        temp_dir.path().to_path_buf(),
        persistence.clone(),
        Duration::from_millis(200),
        None, // No PID for test
    );

    // Simulate Core marking instance as "completed" BEFORE timeout fires
    tokio::time::sleep(Duration::from_millis(50)).await;
    persistence
        .complete_instance(&instance_id, Some(b"success"), None)
        .await
        .expect("Failed to complete instance");

    // Wait for the timeout to fire
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Verify the instance status is still "completed" (not overwritten by timeout)
    let instance = persistence
        .get_instance(&instance_id)
        .await
        .expect("Failed to get instance")
        .expect("Instance not found");

    assert_eq!(
        instance.status, "completed",
        "Status should remain 'completed' even after timeout fires"
    );
    assert!(
        instance.error.is_none() || !instance.error.as_ref().unwrap().contains("timed out"),
        "Should not have timeout error when completed first"
    );

    // Cleanup
    cleanup(&pool, Some(&instance_id), None).await;
}
