// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Tests for management_handlers module.

mod common;

use runtara_core::management_handlers::{
    ManagementHandlerState, handle_get_instance_status, handle_health_check, handle_list_instances,
    handle_send_signal, map_signal_type, map_status,
};
use runtara_core::persistence::PostgresPersistence;
use runtara_protocol::management_proto::{
    GetInstanceStatusRequest, HealthCheckRequest, InstanceStatus, ListInstancesRequest,
    SendSignalRequest, SignalType,
};
use sqlx::PgPool;
use std::time::Duration;
use uuid::Uuid;

/// Helper macro to skip tests if database URL is not set.
macro_rules! skip_if_no_db {
    () => {
        if std::env::var("TEST_DATABASE_URL").is_err() {
            eprintln!("Skipping test: TEST_DATABASE_URL not set");
            return;
        }
    };
}

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Get a database pool for testing
async fn get_test_pool() -> Option<PgPool> {
    let database_url = std::env::var("TEST_DATABASE_URL").ok()?;
    let pool = PgPool::connect(&database_url).await.ok()?;
    MIGRATOR.run(&pool).await.ok()?;
    Some(pool)
}

/// Create test handler state
fn create_test_state(pool: PgPool) -> ManagementHandlerState {
    let persistence = std::sync::Arc::new(PostgresPersistence::new(pool));
    ManagementHandlerState::new(persistence)
}

/// Create a test instance directly in the database
async fn create_test_instance(pool: &PgPool, instance_id: &str, tenant_id: &str, status: &str) {
    let definition_id = Uuid::new_v4();
    // Build query dynamically to cast status to the instance_status enum type
    let query = format!(
        r#"
        INSERT INTO instances (instance_id, tenant_id, definition_id, definition_version, status, created_at)
        VALUES ($1, $2, $3, 1, '{}'::instance_status, NOW())
        ON CONFLICT (instance_id) DO UPDATE SET status = '{}'::instance_status
        "#,
        status, status
    );
    sqlx::query(&query)
        .bind(instance_id)
        .bind(tenant_id)
        .bind(definition_id)
        .execute(pool)
        .await
        .expect("Failed to create test instance");
}

/// Clean up test instance
async fn cleanup_instance(pool: &PgPool, instance_id: &str) {
    sqlx::query("DELETE FROM pending_signals WHERE instance_id = $1")
        .bind(instance_id)
        .execute(pool)
        .await
        .ok();
    sqlx::query("DELETE FROM instance_events WHERE instance_id = $1")
        .bind(instance_id)
        .execute(pool)
        .await
        .ok();
    sqlx::query("DELETE FROM checkpoints WHERE instance_id = $1")
        .bind(instance_id)
        .execute(pool)
        .await
        .ok();
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

// ============================================================================
// ManagementHandlerState Tests
// ============================================================================

#[tokio::test]
async fn test_management_handler_state_creation() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        return;
    };

    let state = create_test_state(pool);
    assert!(!state.version.is_empty());
    assert!(state.uptime_ms() >= 0);
}

#[tokio::test]
async fn test_management_handler_state_uptime() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        return;
    };

    let state = create_test_state(pool);
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
        return;
    };

    let state = create_test_state(pool);
    let request = HealthCheckRequest {};

    let response = handle_health_check(&state, request)
        .await
        .expect("Health check should succeed");

    assert!(response.healthy);
    assert!(!response.version.is_empty());
    // uptime_ms and active_instances are u64, they're always >= 0 by type definition
    let _ = response.uptime_ms;
    let _ = response.active_instances;
}

// ============================================================================
// Send Signal Tests
// ============================================================================

#[tokio::test]
async fn test_send_signal_instance_not_found() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        return;
    };

    let state = create_test_state(pool);
    let request = SendSignalRequest {
        instance_id: "nonexistent-instance".to_string(),
        signal_type: SignalType::SignalCancel.into(),
        payload: vec![],
    };

    let response = handle_send_signal(&state, request)
        .await
        .expect("Handler should not error");

    assert!(!response.success);
    assert!(response.error.contains("not found"));
}

#[tokio::test]
async fn test_send_signal_to_running_instance() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        return;
    };

    let instance_id = Uuid::new_v4().to_string();
    create_test_instance(&pool, &instance_id, "test-tenant", "running").await;

    let state = create_test_state(pool.clone());
    let request = SendSignalRequest {
        instance_id: instance_id.clone(),
        signal_type: SignalType::SignalCancel.into(),
        payload: vec![],
    };

    let response = handle_send_signal(&state, request).await.unwrap();

    assert!(response.success, "Error: {}", response.error);

    // Verify signal was stored
    let signal: Option<(String,)> =
        sqlx::query_as("SELECT signal_type::text FROM pending_signals WHERE instance_id = $1")
            .bind(&instance_id)
            .fetch_optional(&pool)
            .await
            .unwrap();

    assert!(signal.is_some());
    assert_eq!(signal.unwrap().0, "cancel");

    cleanup_instance(&pool, &instance_id).await;
}

#[tokio::test]
async fn test_send_signal_to_suspended_instance() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        return;
    };

    let instance_id = Uuid::new_v4().to_string();
    create_test_instance(&pool, &instance_id, "test-tenant", "suspended").await;

    let state = create_test_state(pool.clone());
    let request = SendSignalRequest {
        instance_id: instance_id.clone(),
        signal_type: SignalType::SignalResume.into(),
        payload: b"resume-payload".to_vec(),
    };

    let response = handle_send_signal(&state, request).await.unwrap();

    assert!(response.success);

    cleanup_instance(&pool, &instance_id).await;
}

#[tokio::test]
async fn test_send_signal_to_terminal_state() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        return;
    };

    let instance_id = Uuid::new_v4().to_string();
    create_test_instance(&pool, &instance_id, "test-tenant", "completed").await;

    let state = create_test_state(pool.clone());
    let request = SendSignalRequest {
        instance_id: instance_id.clone(),
        signal_type: SignalType::SignalCancel.into(),
        payload: vec![],
    };

    let response = handle_send_signal(&state, request).await.unwrap();

    assert!(!response.success);
    assert!(response.error.contains("terminal state"));

    cleanup_instance(&pool, &instance_id).await;
}

#[tokio::test]
async fn test_send_pause_signal() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        return;
    };

    let instance_id = Uuid::new_v4().to_string();
    create_test_instance(&pool, &instance_id, "test-tenant", "running").await;

    let state = create_test_state(pool.clone());
    let request = SendSignalRequest {
        instance_id: instance_id.clone(),
        signal_type: SignalType::SignalPause.into(),
        payload: vec![],
    };

    let response = handle_send_signal(&state, request).await.unwrap();

    assert!(response.success);

    // Verify pause signal was stored
    let signal: Option<(String,)> =
        sqlx::query_as("SELECT signal_type::text FROM pending_signals WHERE instance_id = $1")
            .bind(&instance_id)
            .fetch_optional(&pool)
            .await
            .unwrap();

    assert_eq!(signal.unwrap().0, "pause");

    cleanup_instance(&pool, &instance_id).await;
}

// ============================================================================
// Get Instance Status Tests
// ============================================================================

#[tokio::test]
async fn test_get_instance_status_not_found() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        return;
    };

    let state = create_test_state(pool);
    let request = GetInstanceStatusRequest {
        instance_id: "nonexistent".to_string(),
    };

    let response = handle_get_instance_status(&state, request).await.unwrap();

    assert_eq!(response.status, i32::from(InstanceStatus::StatusUnknown));
    assert!(response.error.is_some());
    assert!(response.error.unwrap().contains("not found"));
}

#[tokio::test]
async fn test_get_instance_status_running() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        return;
    };

    let instance_id = Uuid::new_v4().to_string();
    create_test_instance(&pool, &instance_id, "test-tenant", "running").await;

    // Update started_at
    sqlx::query("UPDATE instances SET started_at = NOW() WHERE instance_id = $1")
        .bind(&instance_id)
        .execute(&pool)
        .await
        .unwrap();

    let state = create_test_state(pool.clone());
    let request = GetInstanceStatusRequest {
        instance_id: instance_id.clone(),
    };

    let response = handle_get_instance_status(&state, request).await.unwrap();

    assert_eq!(response.instance_id, instance_id);
    assert_eq!(response.status, i32::from(InstanceStatus::StatusRunning));
    assert!(response.started_at_ms > 0);
    assert!(response.error.is_none());

    cleanup_instance(&pool, &instance_id).await;
}

#[tokio::test]
async fn test_get_instance_status_completed_with_output() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        return;
    };

    let instance_id = Uuid::new_v4().to_string();
    create_test_instance(&pool, &instance_id, "test-tenant", "completed").await;

    // Update with output
    sqlx::query(
        r#"UPDATE instances SET
            started_at = NOW() - INTERVAL '1 minute',
            finished_at = NOW(),
            output = '{"result": "success"}'
        WHERE instance_id = $1"#,
    )
    .bind(&instance_id)
    .execute(&pool)
    .await
    .unwrap();

    let state = create_test_state(pool.clone());
    let request = GetInstanceStatusRequest {
        instance_id: instance_id.clone(),
    };

    let response = handle_get_instance_status(&state, request).await.unwrap();

    assert_eq!(response.status, i32::from(InstanceStatus::StatusCompleted));
    assert!(response.finished_at_ms.is_some());
    assert!(response.output.is_some());

    cleanup_instance(&pool, &instance_id).await;
}

#[tokio::test]
async fn test_get_instance_status_failed_with_error() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        return;
    };

    let instance_id = Uuid::new_v4().to_string();
    create_test_instance(&pool, &instance_id, "test-tenant", "failed").await;

    // Update with error
    sqlx::query(
        r#"UPDATE instances SET
            started_at = NOW() - INTERVAL '1 minute',
            finished_at = NOW(),
            error = 'Connection refused'
        WHERE instance_id = $1"#,
    )
    .bind(&instance_id)
    .execute(&pool)
    .await
    .unwrap();

    let state = create_test_state(pool.clone());
    let request = GetInstanceStatusRequest {
        instance_id: instance_id.clone(),
    };

    let response = handle_get_instance_status(&state, request).await.unwrap();

    assert_eq!(response.status, i32::from(InstanceStatus::StatusFailed));
    assert!(response.error.is_some());
    assert!(response.error.unwrap().contains("Connection refused"));

    cleanup_instance(&pool, &instance_id).await;
}

// ============================================================================
// List Instances Tests
// ============================================================================

#[tokio::test]
async fn test_list_instances_empty() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        return;
    };

    // Use a unique tenant that won't have any instances
    let state = create_test_state(pool);
    let request = ListInstancesRequest {
        tenant_id: Some(format!("nonexistent-tenant-{}", Uuid::new_v4())),
        status: None,
        limit: 100,
        offset: 0,
    };

    let response = handle_list_instances(&state, request).await.unwrap();

    assert!(response.instances.is_empty());
    assert_eq!(response.total_count, 0);
}

#[tokio::test]
async fn test_list_instances_by_tenant() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        return;
    };

    let tenant_id = format!("list-test-tenant-{}", Uuid::new_v4());
    let instance1 = Uuid::new_v4().to_string();
    let instance2 = Uuid::new_v4().to_string();

    create_test_instance(&pool, &instance1, &tenant_id, "running").await;
    create_test_instance(&pool, &instance2, &tenant_id, "pending").await;

    let state = create_test_state(pool.clone());
    let request = ListInstancesRequest {
        tenant_id: Some(tenant_id.clone()),
        status: None,
        limit: 100,
        offset: 0,
    };

    let response = handle_list_instances(&state, request).await.unwrap();

    assert_eq!(response.instances.len(), 2);
    assert_eq!(response.total_count, 2);

    cleanup_instance(&pool, &instance1).await;
    cleanup_instance(&pool, &instance2).await;
}

#[tokio::test]
async fn test_list_instances_by_status() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        return;
    };

    let tenant_id = format!("status-test-tenant-{}", Uuid::new_v4());
    let running_instance = Uuid::new_v4().to_string();
    let pending_instance = Uuid::new_v4().to_string();

    create_test_instance(&pool, &running_instance, &tenant_id, "running").await;
    create_test_instance(&pool, &pending_instance, &tenant_id, "pending").await;

    let state = create_test_state(pool.clone());

    // Filter by running status (status = 2)
    let request = ListInstancesRequest {
        tenant_id: Some(tenant_id.clone()),
        status: Some(2), // StatusRunning
        limit: 100,
        offset: 0,
    };

    let response = handle_list_instances(&state, request).await.unwrap();

    assert_eq!(response.instances.len(), 1);
    assert_eq!(
        response.instances[0].status,
        i32::from(InstanceStatus::StatusRunning)
    );

    cleanup_instance(&pool, &running_instance).await;
    cleanup_instance(&pool, &pending_instance).await;
}

#[tokio::test]
async fn test_list_instances_with_limit() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        return;
    };

    let tenant_id = format!("limit-test-tenant-{}", Uuid::new_v4());
    let mut instance_ids = Vec::new();

    // Create 5 instances
    for _ in 0..5 {
        let id = Uuid::new_v4().to_string();
        create_test_instance(&pool, &id, &tenant_id, "pending").await;
        instance_ids.push(id);
    }

    let state = create_test_state(pool.clone());
    let request = ListInstancesRequest {
        tenant_id: Some(tenant_id),
        status: None,
        limit: 3, // Only get 3
        offset: 0,
    };

    let response = handle_list_instances(&state, request).await.unwrap();

    assert_eq!(response.instances.len(), 3);

    for id in instance_ids {
        cleanup_instance(&pool, &id).await;
    }
}

#[tokio::test]
async fn test_list_instances_with_offset() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        return;
    };

    let tenant_id = format!("offset-test-tenant-{}", Uuid::new_v4());
    let mut instance_ids = Vec::new();

    // Create 5 instances
    for _ in 0..5 {
        let id = Uuid::new_v4().to_string();
        create_test_instance(&pool, &id, &tenant_id, "pending").await;
        instance_ids.push(id);
    }

    let state = create_test_state(pool.clone());

    // Get all first
    let all_request = ListInstancesRequest {
        tenant_id: Some(tenant_id.clone()),
        status: None,
        limit: 100,
        offset: 0,
    };
    let all_response = handle_list_instances(&state, all_request).await.unwrap();

    // Get with offset
    let request = ListInstancesRequest {
        tenant_id: Some(tenant_id),
        status: None,
        limit: 100,
        offset: 2, // Skip 2
    };
    let response = handle_list_instances(&state, request).await.unwrap();

    assert_eq!(
        response.instances.len(),
        all_response.instances.len().saturating_sub(2)
    );

    for id in instance_ids {
        cleanup_instance(&pool, &id).await;
    }
}

// ============================================================================
// Helper Function Tests (Unit tests)
// ============================================================================

#[test]
fn test_map_signal_type() {
    assert_eq!(map_signal_type(SignalType::SignalCancel), "cancel");
    assert_eq!(map_signal_type(SignalType::SignalPause), "pause");
    assert_eq!(map_signal_type(SignalType::SignalResume), "resume");
}

#[test]
fn test_map_status() {
    assert_eq!(map_status("pending"), InstanceStatus::StatusPending);
    assert_eq!(map_status("running"), InstanceStatus::StatusRunning);
    assert_eq!(map_status("suspended"), InstanceStatus::StatusSuspended);
    assert_eq!(map_status("completed"), InstanceStatus::StatusCompleted);
    assert_eq!(map_status("failed"), InstanceStatus::StatusFailed);
    assert_eq!(map_status("cancelled"), InstanceStatus::StatusCancelled);
    assert_eq!(map_status("unknown"), InstanceStatus::StatusUnknown);
    assert_eq!(map_status("anything-else"), InstanceStatus::StatusUnknown);
}
