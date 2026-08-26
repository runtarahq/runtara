// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Tests for container_registry module.

mod common;

use chrono::Utc;
use runtara_environment::container_registry::{ContainerInfo, ContainerRegistry};
use sqlx::PgPool;
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

/// Get a database pool for testing
async fn get_test_pool() -> PgPool {
    let database_url = std::env::var("TEST_ENVIRONMENT_DATABASE_URL")
        .or_else(|_| std::env::var("RUNTARA_ENVIRONMENT_DATABASE_URL"))
        .expect("db-integration-tests requires an environment database URL");
    let pool = PgPool::connect(&database_url)
        .await
        .expect("required environment test database must accept connections");
    runtara_environment::migrations::run(&pool)
        .await
        .expect("required combined core/environment migrations must succeed");
    pool
}

/// Create a test ContainerInfo
fn create_test_container_info(instance_id: &str, tenant_id: &str) -> ContainerInfo {
    ContainerInfo {
        container_id: format!("container-{}", Uuid::new_v4()),
        instance_id: instance_id.to_string(),
        tenant_id: tenant_id.to_string(),
        binary_path: "/usr/bin/test".to_string(),
        started_at: Utc::now(),
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
    sqlx::query("DELETE FROM container_heartbeats WHERE instance_id = $1")
        .bind(instance_id)
        .execute(pool)
        .await
        .ok();
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
        started_at: Utc::now(),
        timeout_seconds: Some(300),
    };

    assert_eq!(info.container_id, "container-123");
    assert_eq!(info.instance_id, "instance-456");
    assert_eq!(info.tenant_id, "tenant-789");
    assert_eq!(info.binary_path, "/usr/bin/test");
    assert_eq!(info.timeout_seconds, Some(300));
}

#[test]
fn test_container_info_optional_fields() {
    let info = ContainerInfo {
        container_id: "c1".to_string(),
        instance_id: "i1".to_string(),
        tenant_id: "t1".to_string(),
        binary_path: "/bin/test".to_string(),
        started_at: Utc::now(),
        timeout_seconds: None,
    };

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
    let pool = get_test_pool().await;

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
    let pool = get_test_pool().await;

    let registry = ContainerRegistry::new(pool.clone());
    let instance_id = Uuid::new_v4().to_string();
    let mut info = create_test_container_info(&instance_id, "tenant-1");

    // Register first time
    registry.register(&info).await.expect("Failed to register");

    // Update and re-register (upsert)
    info.binary_path = "/new/path".to_string();
    registry
        .register(&info)
        .await
        .expect("Failed to re-register");

    // Verify update
    let retrieved = registry.get(&instance_id).await.unwrap().unwrap();
    assert_eq!(retrieved.binary_path, "/new/path");

    cleanup_instance(&pool, &instance_id).await;
}

#[tokio::test]
async fn test_unregister() {
    skip_if_no_db!();
    let pool = get_test_pool().await;

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
async fn test_list_all_registered() {
    skip_if_no_db!();
    let pool = get_test_pool().await;

    let registry = ContainerRegistry::new(pool.clone());

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

    // Assert only on this test's own rows. A global count is racy: sibling
    // tests register and clean up against the same table concurrently, so
    // `len()` can drop between the two reads.
    let all = registry.list_all_registered().await.unwrap();
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
    let pool = get_test_pool().await;

    let registry = ContainerRegistry::new(pool.clone());
    let result = registry
        .get("nonexistent-instance-id")
        .await
        .expect("Query should succeed");
    assert!(result.is_none());
}

// ============================================================================
// Cleanup Tests
// ============================================================================

#[tokio::test]
async fn test_cleanup_single_container() {
    skip_if_no_db!();
    let pool = get_test_pool().await;

    let registry = ContainerRegistry::new(pool.clone());
    let instance_id = Uuid::new_v4().to_string();

    // cleanup() clears three tables in one transaction. Only container_registry
    // still has a writer in the codebase, so seed the other two directly — the
    // point of the test is that cleanup() empties all three, whoever wrote them.
    let info = create_test_container_info(&instance_id, "tenant-1");
    registry.register(&info).await.unwrap();
    sqlx::query(
        "INSERT INTO container_heartbeats (instance_id, last_heartbeat) VALUES ($1, NOW())",
    )
    .bind(&instance_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO container_status (instance_id, status, updated_at) VALUES ($1, $2, NOW())",
    )
    .bind(&instance_id)
    .bind(serde_json::json!({"status": "running"}))
    .execute(&pool)
    .await
    .unwrap();

    for table in TRACKING_TABLES {
        assert_eq!(
            row_count(&pool, table, &instance_id).await,
            1,
            "{table} should be seeded before cleanup"
        );
    }

    registry.cleanup(&instance_id).await.unwrap();

    for table in TRACKING_TABLES {
        assert_eq!(
            row_count(&pool, table, &instance_id).await,
            0,
            "{table} should be empty after cleanup"
        );
    }
}

/// Every table `ContainerRegistry::cleanup` is responsible for emptying.
const TRACKING_TABLES: [&str; 3] = [
    "container_registry",
    "container_status",
    "container_heartbeats",
];

async fn row_count(pool: &PgPool, table: &str, instance_id: &str) -> i64 {
    // Table names come from the const above, never from test input.
    sqlx::query_scalar::<_, i64>(&format!(
        "SELECT COUNT(*) FROM {table} WHERE instance_id = $1"
    ))
    .bind(instance_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

// ============================================================================
// Edge Cases
// ============================================================================

#[tokio::test]
async fn test_operations_on_nonexistent_container() {
    skip_if_no_db!();
    let pool = get_test_pool().await;

    let registry = ContainerRegistry::new(pool.clone());
    let instance_id = "nonexistent-instance";

    // All these should succeed (no error) but have no effect
    registry.unregister(instance_id).await.unwrap();
    registry.cleanup(instance_id).await.unwrap();
}

#[tokio::test]
async fn test_container_with_no_optional_fields() {
    skip_if_no_db!();
    let pool = get_test_pool().await;

    let registry = ContainerRegistry::new(pool.clone());
    let instance_id = Uuid::new_v4().to_string();

    let info = ContainerInfo {
        container_id: format!("c-{}", instance_id),
        instance_id: instance_id.clone(),
        tenant_id: "tenant".to_string(),
        binary_path: "/bin/test".to_string(),
        started_at: Utc::now(),
        timeout_seconds: None,
    };

    registry.register(&info).await.unwrap();

    let retrieved = registry.get(&instance_id).await.unwrap().unwrap();
    assert!(retrieved.timeout_seconds.is_none());

    cleanup_instance(&pool, &instance_id).await;
}
