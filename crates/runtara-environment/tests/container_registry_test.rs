// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Tests for container_registry module.

mod common;

use chrono::Utc;
use runtara_environment::container_registry::{ContainerInfo, ContainerRegistry, ContainerStatus};
use sqlx::PgPool;
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

/// Create a test ContainerInfo
fn create_test_container_info(instance_id: &str, tenant_id: &str) -> ContainerInfo {
    ContainerInfo {
        container_id: format!("container-{}", Uuid::new_v4()),
        instance_id: instance_id.to_string(),
        tenant_id: tenant_id.to_string(),
        binary_path: "/usr/bin/test".to_string(),
        bundle_path: Some("/tmp/bundle".to_string()),
        started_at: Utc::now(),
        pid: None,
        timeout_seconds: Some(300),
    }
}

/// Clean up test data for a specific instance
async fn cleanup_instance(pool: &PgPool, instance_id: &str) {
    sqlx::query("DELETE FROM container_registry WHERE instance_id = $1")
        .bind(instance_id)
        .execute(pool)
        .await
        .ok();
    sqlx::query("DELETE FROM container_status WHERE instance_id = $1")
        .bind(instance_id)
        .execute(pool)
        .await
        .ok();
    sqlx::query("DELETE FROM container_cancellations WHERE instance_id = $1")
        .bind(instance_id)
        .execute(pool)
        .await
        .ok();
    sqlx::query("DELETE FROM container_heartbeats WHERE instance_id = $1")
        .bind(instance_id)
        .execute(pool)
        .await
        .ok();
}

// ============================================================================
// ContainerStatus Tests (Unit tests - no DB required)
// ============================================================================

#[test]
fn test_container_status_is_terminal() {
    let running = ContainerStatus::Running {
        updated_at: Utc::now(),
    };
    assert!(!running.is_terminal());

    let completed = ContainerStatus::Completed {
        updated_at: Utc::now(),
        output: None,
    };
    assert!(completed.is_terminal());

    let failed = ContainerStatus::Failed {
        updated_at: Utc::now(),
        error: "test error".to_string(),
    };
    assert!(failed.is_terminal());

    let cancelled = ContainerStatus::Cancelled {
        updated_at: Utc::now(),
    };
    assert!(cancelled.is_terminal());
}

#[test]
fn test_container_status_status_str() {
    let running = ContainerStatus::Running {
        updated_at: Utc::now(),
    };
    assert_eq!(running.status_str(), "running");

    let completed = ContainerStatus::Completed {
        updated_at: Utc::now(),
        output: None,
    };
    assert_eq!(completed.status_str(), "completed");

    let failed = ContainerStatus::Failed {
        updated_at: Utc::now(),
        error: "error".to_string(),
    };
    assert_eq!(failed.status_str(), "failed");

    let cancelled = ContainerStatus::Cancelled {
        updated_at: Utc::now(),
    };
    assert_eq!(cancelled.status_str(), "cancelled");
}

#[test]
fn test_container_status_updated_at() {
    let now = Utc::now();

    let running = ContainerStatus::Running { updated_at: now };
    assert_eq!(running.updated_at(), now);

    let completed = ContainerStatus::Completed {
        updated_at: now,
        output: Some(serde_json::json!({"result": "ok"})),
    };
    assert_eq!(completed.updated_at(), now);

    let failed = ContainerStatus::Failed {
        updated_at: now,
        error: "error".to_string(),
    };
    assert_eq!(failed.updated_at(), now);

    let cancelled = ContainerStatus::Cancelled { updated_at: now };
    assert_eq!(cancelled.updated_at(), now);
}

#[test]
fn test_container_status_serialization() {
    let running = ContainerStatus::Running {
        updated_at: Utc::now(),
    };
    let json = serde_json::to_string(&running).unwrap();
    assert!(json.contains("\"status\":\"running\""));

    let completed = ContainerStatus::Completed {
        updated_at: Utc::now(),
        output: Some(serde_json::json!({"value": 42})),
    };
    let json = serde_json::to_string(&completed).unwrap();
    assert!(json.contains("\"status\":\"completed\""));
    assert!(json.contains("\"value\":42"));

    let failed = ContainerStatus::Failed {
        updated_at: Utc::now(),
        error: "test error".to_string(),
    };
    let json = serde_json::to_string(&failed).unwrap();
    assert!(json.contains("\"status\":\"failed\""));
    assert!(json.contains("\"error\":\"test error\""));
}

#[test]
fn test_container_status_deserialization() {
    let json = r#"{"status":"running","updated_at":"2024-01-01T00:00:00Z"}"#;
    let status: ContainerStatus = serde_json::from_str(json).unwrap();
    assert!(matches!(status, ContainerStatus::Running { .. }));

    let json = r#"{"status":"completed","updated_at":"2024-01-01T00:00:00Z","output":{"result":"success"}}"#;
    let status: ContainerStatus = serde_json::from_str(json).unwrap();
    assert!(matches!(
        status,
        ContainerStatus::Completed {
            output: Some(_),
            ..
        }
    ));

    let json =
        r#"{"status":"failed","updated_at":"2024-01-01T00:00:00Z","error":"connection timeout"}"#;
    let status: ContainerStatus = serde_json::from_str(json).unwrap();
    if let ContainerStatus::Failed { error, .. } = status {
        assert_eq!(error, "connection timeout");
    } else {
        panic!("Expected Failed status");
    }

    let json = r#"{"status":"cancelled","updated_at":"2024-01-01T00:00:00Z"}"#;
    let status: ContainerStatus = serde_json::from_str(json).unwrap();
    assert!(matches!(status, ContainerStatus::Cancelled { .. }));
}

// ============================================================================
// ContainerInfo Tests
// ============================================================================

#[test]
fn test_container_info_creation() {
    let info = ContainerInfo {
        container_id: "container-123".to_string(),
        instance_id: "instance-456".to_string(),
        tenant_id: "tenant-789".to_string(),
        binary_path: "/usr/bin/test".to_string(),
        bundle_path: Some("/tmp/bundle".to_string()),
        started_at: Utc::now(),
        pid: Some(12345),
        timeout_seconds: Some(300),
    };

    assert_eq!(info.container_id, "container-123");
    assert_eq!(info.instance_id, "instance-456");
    assert_eq!(info.tenant_id, "tenant-789");
    assert_eq!(info.binary_path, "/usr/bin/test");
    assert_eq!(info.bundle_path, Some("/tmp/bundle".to_string()));
    assert_eq!(info.pid, Some(12345));
    assert_eq!(info.timeout_seconds, Some(300));
}

#[test]
fn test_container_info_optional_fields() {
    let info = ContainerInfo {
        container_id: "c1".to_string(),
        instance_id: "i1".to_string(),
        tenant_id: "t1".to_string(),
        binary_path: "/bin/test".to_string(),
        bundle_path: None,
        started_at: Utc::now(),
        pid: None,
        timeout_seconds: None,
    };

    assert!(info.bundle_path.is_none());
    assert!(info.pid.is_none());
    assert!(info.timeout_seconds.is_none());
}

#[test]
fn test_container_info_serialization() {
    let info = create_test_container_info("inst-1", "tenant-1");
    let json = serde_json::to_string(&info).unwrap();
    assert!(json.contains("container_id"));
    assert!(json.contains("instance_id"));
    assert!(json.contains("tenant_id"));

    // Deserialize back
    let parsed: ContainerInfo = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.instance_id, info.instance_id);
    assert_eq!(parsed.tenant_id, info.tenant_id);
}

// ============================================================================
// ContainerRegistry Database Tests
// ============================================================================

#[tokio::test]
async fn test_register_and_get() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let registry = ContainerRegistry::new(pool.clone());
    let instance_id = Uuid::new_v4().to_string();
    let info = create_test_container_info(&instance_id, "test-tenant");

    // Register
    registry.register(&info).await.expect("Failed to register");

    // Get
    let retrieved = registry
        .get(&instance_id)
        .await
        .expect("Failed to get")
        .expect("Should find container");

    assert_eq!(retrieved.instance_id, instance_id);
    assert_eq!(retrieved.tenant_id, "test-tenant");
    assert_eq!(retrieved.binary_path, "/usr/bin/test");

    cleanup_instance(&pool, &instance_id).await;
}

#[tokio::test]
async fn test_register_upsert() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let registry = ContainerRegistry::new(pool.clone());
    let instance_id = Uuid::new_v4().to_string();
    let mut info = create_test_container_info(&instance_id, "tenant-1");

    // Register first time
    registry.register(&info).await.expect("Failed to register");

    // Update and re-register (upsert)
    info.binary_path = "/new/path".to_string();
    info.pid = Some(99999);
    registry
        .register(&info)
        .await
        .expect("Failed to re-register");

    // Verify update
    let retrieved = registry.get(&instance_id).await.unwrap().unwrap();
    assert_eq!(retrieved.binary_path, "/new/path");
    assert_eq!(retrieved.pid, Some(99999));

    cleanup_instance(&pool, &instance_id).await;
}

#[tokio::test]
async fn test_unregister() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let registry = ContainerRegistry::new(pool.clone());
    let instance_id = Uuid::new_v4().to_string();
    let info = create_test_container_info(&instance_id, "tenant-1");

    registry.register(&info).await.unwrap();
    assert!(registry.get(&instance_id).await.unwrap().is_some());

    registry.unregister(&instance_id).await.unwrap();
    assert!(registry.get(&instance_id).await.unwrap().is_none());

    cleanup_instance(&pool, &instance_id).await;
}

#[tokio::test]
async fn test_list_registered_by_tenant() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let registry = ContainerRegistry::new(pool.clone());

    let instance1 = Uuid::new_v4().to_string();
    let instance2 = Uuid::new_v4().to_string();
    let instance3 = Uuid::new_v4().to_string();

    // Register containers for different tenants
    registry
        .register(&create_test_container_info(&instance1, "tenant-a"))
        .await
        .unwrap();
    registry
        .register(&create_test_container_info(&instance2, "tenant-a"))
        .await
        .unwrap();
    registry
        .register(&create_test_container_info(&instance3, "tenant-b"))
        .await
        .unwrap();

    // List by tenant
    let tenant_a_containers = registry.list_registered("tenant-a").await.unwrap();
    assert_eq!(tenant_a_containers.len(), 2);

    let tenant_b_containers = registry.list_registered("tenant-b").await.unwrap();
    assert_eq!(tenant_b_containers.len(), 1);

    let tenant_c_containers = registry.list_registered("tenant-c").await.unwrap();
    assert!(tenant_c_containers.is_empty());

    cleanup_instance(&pool, &instance1).await;
    cleanup_instance(&pool, &instance2).await;
    cleanup_instance(&pool, &instance3).await;
}

#[tokio::test]
async fn test_list_all_registered() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let registry = ContainerRegistry::new(pool.clone());

    // Get initial count (may include containers from other parallel tests)
    let initial_count = registry.list_all_registered().await.unwrap().len();

    let instance1 = Uuid::new_v4().to_string();
    let instance2 = Uuid::new_v4().to_string();

    registry
        .register(&create_test_container_info(&instance1, "tenant-list-1"))
        .await
        .unwrap();
    registry
        .register(&create_test_container_info(&instance2, "tenant-list-2"))
        .await
        .unwrap();

    // Should have 2 more than initial count
    let all = registry.list_all_registered().await.unwrap();
    assert!(
        all.len() >= initial_count + 2,
        "Should have at least 2 more containers after registering"
    );

    // Verify our specific containers are in the list
    assert!(
        all.iter().any(|c| c.instance_id == instance1),
        "instance1 should be in the list"
    );
    assert!(
        all.iter().any(|c| c.instance_id == instance2),
        "instance2 should be in the list"
    );

    cleanup_instance(&pool, &instance1).await;
    cleanup_instance(&pool, &instance2).await;
}

#[tokio::test]
async fn test_get_nonexistent() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let registry = ContainerRegistry::new(pool.clone());
    let result = registry
        .get("nonexistent-instance-id")
        .await
        .expect("Query should succeed");
    assert!(result.is_none());
}

#[tokio::test]
async fn test_update_pid() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let registry = ContainerRegistry::new(pool.clone());
    let instance_id = Uuid::new_v4().to_string();
    let info = create_test_container_info(&instance_id, "tenant-1");

    registry.register(&info).await.unwrap();

    // Verify no PID initially
    let retrieved = registry.get(&instance_id).await.unwrap().unwrap();
    assert!(retrieved.pid.is_none());

    // Update PID
    registry.update_pid(&instance_id, 12345).await.unwrap();

    // Verify PID updated
    let retrieved = registry.get(&instance_id).await.unwrap().unwrap();
    assert_eq!(retrieved.pid, Some(12345));

    cleanup_instance(&pool, &instance_id).await;
}

// ============================================================================
// Cancellation Tests
// ============================================================================

#[tokio::test]
async fn test_request_cancellation() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let registry = ContainerRegistry::new(pool.clone());
    let instance_id = Uuid::new_v4().to_string();

    // Request cancellation
    registry
        .request_cancellation(&instance_id, Duration::from_secs(30), "test cancellation")
        .await
        .unwrap();

    // Check cancellation
    let cancel = registry
        .check_cancellation(&instance_id)
        .await
        .unwrap()
        .expect("Should have cancellation request");

    assert_eq!(cancel.instance_id, instance_id);
    assert_eq!(cancel.grace_period_seconds, 30);
    assert_eq!(cancel.reason, "test cancellation");

    cleanup_instance(&pool, &instance_id).await;
}

#[tokio::test]
async fn test_cancellation_upsert() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let registry = ContainerRegistry::new(pool.clone());
    let instance_id = Uuid::new_v4().to_string();

    // Request first cancellation
    registry
        .request_cancellation(&instance_id, Duration::from_secs(30), "first reason")
        .await
        .unwrap();

    // Request second cancellation (should update)
    registry
        .request_cancellation(&instance_id, Duration::from_secs(60), "second reason")
        .await
        .unwrap();

    // Check - should have updated values
    let cancel = registry
        .check_cancellation(&instance_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cancel.grace_period_seconds, 60);
    assert_eq!(cancel.reason, "second reason");

    cleanup_instance(&pool, &instance_id).await;
}

#[tokio::test]
async fn test_check_cancellation_none() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let registry = ContainerRegistry::new(pool.clone());
    let result = registry
        .check_cancellation("nonexistent")
        .await
        .expect("Query should succeed");
    assert!(result.is_none());
}

#[tokio::test]
async fn test_clear_cancellation() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let registry = ContainerRegistry::new(pool.clone());
    let instance_id = Uuid::new_v4().to_string();

    registry
        .request_cancellation(&instance_id, Duration::from_secs(10), "reason")
        .await
        .unwrap();

    assert!(
        registry
            .check_cancellation(&instance_id)
            .await
            .unwrap()
            .is_some()
    );

    registry.clear_cancellation(&instance_id).await.unwrap();

    assert!(
        registry
            .check_cancellation(&instance_id)
            .await
            .unwrap()
            .is_none()
    );

    cleanup_instance(&pool, &instance_id).await;
}

// ============================================================================
// Status Reporting Tests
// ============================================================================

#[tokio::test]
async fn test_report_and_get_status_running() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let registry = ContainerRegistry::new(pool.clone());
    let instance_id = Uuid::new_v4().to_string();

    let status = ContainerStatus::Running {
        updated_at: Utc::now(),
    };

    registry.report_status(&instance_id, &status).await.unwrap();

    let retrieved = registry
        .get_status(&instance_id)
        .await
        .unwrap()
        .expect("Should have status");

    assert!(matches!(retrieved, ContainerStatus::Running { .. }));
    assert_eq!(retrieved.status_str(), "running");

    cleanup_instance(&pool, &instance_id).await;
}

#[tokio::test]
async fn test_report_and_get_status_completed() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let registry = ContainerRegistry::new(pool.clone());
    let instance_id = Uuid::new_v4().to_string();

    let status = ContainerStatus::Completed {
        updated_at: Utc::now(),
        output: Some(serde_json::json!({"result": "success", "count": 42})),
    };

    registry.report_status(&instance_id, &status).await.unwrap();

    let retrieved = registry.get_status(&instance_id).await.unwrap().unwrap();

    if let ContainerStatus::Completed { output, .. } = retrieved {
        assert!(output.is_some());
        let output = output.unwrap();
        assert_eq!(output["result"], "success");
        assert_eq!(output["count"], 42);
    } else {
        panic!("Expected Completed status");
    }

    cleanup_instance(&pool, &instance_id).await;
}

#[tokio::test]
async fn test_report_and_get_status_failed() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let registry = ContainerRegistry::new(pool.clone());
    let instance_id = Uuid::new_v4().to_string();

    let status = ContainerStatus::Failed {
        updated_at: Utc::now(),
        error: "Connection refused".to_string(),
    };

    registry.report_status(&instance_id, &status).await.unwrap();

    let retrieved = registry.get_status(&instance_id).await.unwrap().unwrap();

    if let ContainerStatus::Failed { error, .. } = retrieved {
        assert_eq!(error, "Connection refused");
    } else {
        panic!("Expected Failed status");
    }

    cleanup_instance(&pool, &instance_id).await;
}

#[tokio::test]
async fn test_report_and_get_status_cancelled() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let registry = ContainerRegistry::new(pool.clone());
    let instance_id = Uuid::new_v4().to_string();

    let status = ContainerStatus::Cancelled {
        updated_at: Utc::now(),
    };

    registry.report_status(&instance_id, &status).await.unwrap();

    let retrieved = registry.get_status(&instance_id).await.unwrap().unwrap();
    assert!(matches!(retrieved, ContainerStatus::Cancelled { .. }));

    cleanup_instance(&pool, &instance_id).await;
}

#[tokio::test]
async fn test_status_upsert() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let registry = ContainerRegistry::new(pool.clone());
    let instance_id = Uuid::new_v4().to_string();

    // Report running
    let running = ContainerStatus::Running {
        updated_at: Utc::now(),
    };
    registry
        .report_status(&instance_id, &running)
        .await
        .unwrap();

    // Update to completed
    let completed = ContainerStatus::Completed {
        updated_at: Utc::now(),
        output: None,
    };
    registry
        .report_status(&instance_id, &completed)
        .await
        .unwrap();

    // Should be completed now
    let retrieved = registry.get_status(&instance_id).await.unwrap().unwrap();
    assert!(matches!(retrieved, ContainerStatus::Completed { .. }));

    cleanup_instance(&pool, &instance_id).await;
}

#[tokio::test]
async fn test_get_status_none() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let registry = ContainerRegistry::new(pool.clone());
    let result = registry
        .get_status("nonexistent")
        .await
        .expect("Query should succeed");
    assert!(result.is_none());
}

#[tokio::test]
async fn test_clear_status() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let registry = ContainerRegistry::new(pool.clone());
    let instance_id = Uuid::new_v4().to_string();

    let status = ContainerStatus::Running {
        updated_at: Utc::now(),
    };
    registry.report_status(&instance_id, &status).await.unwrap();

    assert!(registry.get_status(&instance_id).await.unwrap().is_some());

    registry.clear_status(&instance_id).await.unwrap();

    assert!(registry.get_status(&instance_id).await.unwrap().is_none());

    cleanup_instance(&pool, &instance_id).await;
}

// ============================================================================
// Heartbeat Tests
// ============================================================================

#[tokio::test]
async fn test_send_heartbeat() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let registry = ContainerRegistry::new(pool.clone());
    let instance_id = Uuid::new_v4().to_string();

    // Initially no heartbeat
    assert!(!registry.has_heartbeat(&instance_id).await.unwrap());
    assert!(
        registry
            .get_heartbeat(&instance_id)
            .await
            .unwrap()
            .is_none()
    );

    // Send heartbeat
    registry.send_heartbeat(&instance_id).await.unwrap();

    // Should have heartbeat now
    assert!(registry.has_heartbeat(&instance_id).await.unwrap());
    assert!(
        registry
            .get_heartbeat(&instance_id)
            .await
            .unwrap()
            .is_some()
    );

    cleanup_instance(&pool, &instance_id).await;
}

#[tokio::test]
async fn test_heartbeat_upsert() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let registry = ContainerRegistry::new(pool.clone());
    let instance_id = Uuid::new_v4().to_string();

    // First heartbeat
    registry.send_heartbeat(&instance_id).await.unwrap();
    let first_ts = registry.get_heartbeat(&instance_id).await.unwrap().unwrap();

    // Wait a bit
    tokio::time::sleep(Duration::from_millis(10)).await;

    // Second heartbeat
    registry.send_heartbeat(&instance_id).await.unwrap();
    let second_ts = registry.get_heartbeat(&instance_id).await.unwrap().unwrap();

    // Timestamp should be updated
    assert!(second_ts >= first_ts);

    cleanup_instance(&pool, &instance_id).await;
}

#[tokio::test]
async fn test_has_heartbeat_recent() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let registry = ContainerRegistry::new(pool.clone());
    let instance_id = Uuid::new_v4().to_string();

    // Send recent heartbeat
    registry.send_heartbeat(&instance_id).await.unwrap();

    // Should be detected as having heartbeat (within 60s window)
    assert!(registry.has_heartbeat(&instance_id).await.unwrap());

    cleanup_instance(&pool, &instance_id).await;
}

#[tokio::test]
async fn test_clear_heartbeat() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let registry = ContainerRegistry::new(pool.clone());
    let instance_id = Uuid::new_v4().to_string();

    registry.send_heartbeat(&instance_id).await.unwrap();
    assert!(registry.has_heartbeat(&instance_id).await.unwrap());

    registry.clear_heartbeat(&instance_id).await.unwrap();
    assert!(!registry.has_heartbeat(&instance_id).await.unwrap());

    cleanup_instance(&pool, &instance_id).await;
}

// ============================================================================
// Cleanup Tests
// ============================================================================

#[tokio::test]
async fn test_cleanup_single_container() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let registry = ContainerRegistry::new(pool.clone());
    let instance_id = Uuid::new_v4().to_string();

    // Set up all data types for a container
    let info = create_test_container_info(&instance_id, "tenant-1");
    registry.register(&info).await.unwrap();
    registry.send_heartbeat(&instance_id).await.unwrap();
    registry
        .request_cancellation(&instance_id, Duration::from_secs(30), "test")
        .await
        .unwrap();
    registry
        .report_status(
            &instance_id,
            &ContainerStatus::Running {
                updated_at: Utc::now(),
            },
        )
        .await
        .unwrap();

    // Verify all exist
    assert!(registry.get(&instance_id).await.unwrap().is_some());
    // Use get_heartbeat instead of has_heartbeat to avoid timing issues
    // has_heartbeat checks if heartbeat is within 60s, which can fail due to clock drift
    assert!(
        registry
            .get_heartbeat(&instance_id)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        registry
            .check_cancellation(&instance_id)
            .await
            .unwrap()
            .is_some()
    );
    assert!(registry.get_status(&instance_id).await.unwrap().is_some());

    // Cleanup
    registry.cleanup(&instance_id).await.unwrap();

    // Verify all gone
    assert!(registry.get(&instance_id).await.unwrap().is_none());
    // Use get_heartbeat to check for absence (consistent with check above)
    assert!(
        registry
            .get_heartbeat(&instance_id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        registry
            .check_cancellation(&instance_id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(registry.get_status(&instance_id).await.unwrap().is_none());
}

#[tokio::test]
async fn test_cleanup_stale() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let registry = ContainerRegistry::new(pool.clone());

    let fresh_instance = Uuid::new_v4().to_string();
    let stale_instance = Uuid::new_v4().to_string();

    // Register both
    registry
        .register(&create_test_container_info(
            &fresh_instance,
            "tenant-cleanup-test",
        ))
        .await
        .unwrap();
    registry
        .register(&create_test_container_info(
            &stale_instance,
            "tenant-cleanup-test",
        ))
        .await
        .unwrap();

    // Only fresh one gets heartbeat
    registry.send_heartbeat(&fresh_instance).await.unwrap();

    // The stale one has no heartbeat at all, so cleanup_stale should remove it
    // cleanup_stale removes ALL containers without heartbeats, so we can only check
    // that our stale instance is removed, not the exact count
    let cleaned_before = registry.cleanup_stale().await.unwrap();

    // Fresh should still exist
    assert!(
        registry.get(&fresh_instance).await.unwrap().is_some(),
        "Fresh container with heartbeat should survive cleanup"
    );

    // Stale should be gone
    assert!(
        registry.get(&stale_instance).await.unwrap().is_none(),
        "Stale container without heartbeat should be cleaned up"
    );

    // At least 1 was cleaned (our stale one), possibly more from other tests
    assert!(
        cleaned_before >= 1,
        "At least 1 stale container should be cleaned"
    );

    cleanup_instance(&pool, &fresh_instance).await;
    cleanup_instance(&pool, &stale_instance).await;
}

// ============================================================================
// Edge Cases
// ============================================================================

#[tokio::test]
async fn test_operations_on_nonexistent_container() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let registry = ContainerRegistry::new(pool.clone());
    let instance_id = "nonexistent-instance";

    // All these should succeed (no error) but have no effect
    registry.unregister(instance_id).await.unwrap();
    registry.update_pid(instance_id, 12345).await.unwrap();
    registry.clear_cancellation(instance_id).await.unwrap();
    registry.clear_status(instance_id).await.unwrap();
    registry.clear_heartbeat(instance_id).await.unwrap();
    registry.cleanup(instance_id).await.unwrap();
}

#[tokio::test]
async fn test_container_with_no_optional_fields() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let registry = ContainerRegistry::new(pool.clone());
    let instance_id = Uuid::new_v4().to_string();

    let info = ContainerInfo {
        container_id: format!("c-{}", instance_id),
        instance_id: instance_id.clone(),
        tenant_id: "tenant".to_string(),
        binary_path: "/bin/test".to_string(),
        bundle_path: None,
        started_at: Utc::now(),
        pid: None,
        timeout_seconds: None,
    };

    registry.register(&info).await.unwrap();

    let retrieved = registry.get(&instance_id).await.unwrap().unwrap();
    assert!(retrieved.bundle_path.is_none());
    assert!(retrieved.pid.is_none());
    assert!(retrieved.timeout_seconds.is_none());

    cleanup_instance(&pool, &instance_id).await;
}
