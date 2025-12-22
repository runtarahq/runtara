// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Tests for environment handlers module.

mod common;

use chrono::Utc;
use runtara_environment::db;
use runtara_environment::handlers::{
    EnvironmentHandlerState, GetCapabilityRequest, RegisterImageRequest, ResumeInstanceRequest,
    StartInstanceRequest, StopInstanceRequest, TestCapabilityRequest, handle_get_capability,
    handle_health_check, handle_list_agents, handle_register_image, handle_resume_instance,
    handle_start_instance, handle_stop_instance, handle_test_capability,
};
use runtara_environment::image_registry::{ImageRegistry, RunnerType};
use runtara_environment::runner::MockRunner;
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
    EnvironmentHandlerState::new(pool, runner, "127.0.0.1:8001".to_string(), data_dir)
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

    db::create_instance(&pool, &instance_id, "test-tenant", &image_id)
        .await
        .unwrap();

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

    db::create_instance(&pool, &instance_id, "test-tenant", &image_id)
        .await
        .unwrap();
    db::update_instance_status(&pool, &instance_id, "running", None)
        .await
        .unwrap();

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

    db::create_instance(&pool, &instance_id, "test-tenant", &image_id)
        .await
        .unwrap();
    db::update_instance_status(&pool, &instance_id, "suspended", None)
        .await
        .unwrap();

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

    db::create_instance(&pool, &instance_id, "test-tenant", &image_id)
        .await
        .unwrap();
    db::update_instance_status(&pool, &instance_id, "suspended", Some("checkpoint-123"))
        .await
        .unwrap();

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
