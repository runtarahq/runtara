// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Tests for heartbeat_monitor module - detecting and failing stale/orphaned instances.

mod common;

use async_trait::async_trait;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use runtara_core::error::CoreError;
use runtara_core::persistence::{
    CheckpointRecord, CustomSignalRecord, EventRecord, InstanceRecord, ListEventsFilter,
    ListStepSummariesFilter, Persistence, SignalRecord, StepSummaryRecord,
};
use runtara_environment::heartbeat_monitor::{HeartbeatMonitor, HeartbeatMonitorConfig};
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
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

/// Create a test instance in Environment's instances table
async fn create_env_instance(
    pool: &PgPool,
    instance_id: &str,
    tenant_id: &str,
    image_id: &str,
    status: &str,
) {
    sqlx::query(
        r#"
        INSERT INTO instances (instance_id, tenant_id, image_id, status, created_at, started_at)
        VALUES ($1, $2, $3, $4, NOW() - INTERVAL '1 hour', NOW() - INTERVAL '1 hour')
        "#,
    )
    .bind(instance_id)
    .bind(tenant_id)
    .bind(image_id)
    .bind(status)
    .execute(pool)
    .await
    .expect("Failed to create test instance");
}

/// Register a container in container_registry
async fn register_container(pool: &PgPool, instance_id: &str, tenant_id: &str, image_id: &str) {
    sqlx::query(
        r#"
        INSERT INTO container_registry (instance_id, tenant_id, image_id, started_at)
        VALUES ($1, $2, $3, NOW() - INTERVAL '30 minutes')
        "#,
    )
    .bind(instance_id)
    .bind(tenant_id)
    .bind(image_id)
    .execute(pool)
    .await
    .expect("Failed to register container");
}

/// Record an event in instance_events table (used by HeartbeatMonitor for activity detection)
async fn record_instance_event(
    pool: &PgPool,
    instance_id: &str,
    tenant_id: &str,
    minutes_ago: i64,
) {
    let event_time = Utc::now() - ChronoDuration::minutes(minutes_ago);
    let event_id = Uuid::new_v4().to_string();
    sqlx::query(
        r#"
        INSERT INTO instance_events (event_id, instance_id, tenant_id, event_type, payload, created_at)
        VALUES ($1, $2, $3, 'heartbeat', $4, $5)
        "#,
    )
    .bind(&event_id)
    .bind(instance_id)
    .bind(tenant_id)
    .bind(b"{}".as_slice())
    .bind(event_time)
    .execute(pool)
    .await
    .expect("Failed to record instance event");
}

/// Clean up test data
async fn cleanup(pool: &PgPool, instance_id: &str) {
    sqlx::query("DELETE FROM instance_events WHERE instance_id = $1")
        .bind(instance_id)
        .execute(pool)
        .await
        .ok();
    sqlx::query("DELETE FROM container_heartbeats WHERE instance_id = $1")
        .bind(instance_id)
        .execute(pool)
        .await
        .ok();
    sqlx::query("DELETE FROM container_registry WHERE instance_id = $1")
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
// Mock Persistence for Testing
// ============================================================================

/// Mock persistence that tracks instances and allows testing orphaned instance detection.
struct MockPersistence {
    instances: Mutex<HashMap<String, InstanceRecord>>,
    #[allow(clippy::type_complexity)]
    completed_instances: Mutex<Vec<(String, Option<Vec<u8>>, Option<String>)>>,
}

impl MockPersistence {
    fn new() -> Self {
        Self {
            instances: Mutex::new(HashMap::new()),
            completed_instances: Mutex::new(Vec::new()),
        }
    }

    fn with_running_instance(
        self,
        instance_id: &str,
        tenant_id: &str,
        started_at: DateTime<Utc>,
    ) -> Self {
        let record = InstanceRecord {
            instance_id: instance_id.to_string(),
            tenant_id: tenant_id.to_string(),
            definition_version: 1,
            status: "running".to_string(),
            checkpoint_id: None,
            attempt: 1,
            max_attempts: 3,
            created_at: started_at - ChronoDuration::minutes(5),
            started_at: Some(started_at),
            finished_at: None,
            output: None,
            error: None,
            sleep_until: None,
            termination_reason: None,
            exit_code: None,
        };
        self.instances
            .lock()
            .unwrap()
            .insert(instance_id.to_string(), record);
        self
    }

    fn get_completed_instances(&self) -> Vec<(String, Option<Vec<u8>>, Option<String>)> {
        self.completed_instances.lock().unwrap().clone()
    }
}

#[async_trait]
impl Persistence for MockPersistence {
    async fn register_instance(
        &self,
        _instance_id: &str,
        _tenant_id: &str,
    ) -> Result<(), CoreError> {
        Ok(())
    }

    async fn get_instance(&self, instance_id: &str) -> Result<Option<InstanceRecord>, CoreError> {
        Ok(self.instances.lock().unwrap().get(instance_id).cloned())
    }

    async fn update_instance_status(
        &self,
        _instance_id: &str,
        _status: &str,
        _started_at: Option<DateTime<Utc>>,
    ) -> Result<(), CoreError> {
        Ok(())
    }

    async fn update_instance_checkpoint(
        &self,
        _instance_id: &str,
        _checkpoint_id: &str,
    ) -> Result<(), CoreError> {
        Ok(())
    }

    async fn complete_instance(
        &self,
        instance_id: &str,
        output: Option<&[u8]>,
        error_message: Option<&str>,
    ) -> Result<(), CoreError> {
        self.completed_instances.lock().unwrap().push((
            instance_id.to_string(),
            output.map(|o| o.to_vec()),
            error_message.map(|e| e.to_string()),
        ));
        // Remove from instances
        self.instances.lock().unwrap().remove(instance_id);
        Ok(())
    }

    async fn save_checkpoint(
        &self,
        _instance_id: &str,
        _checkpoint_id: &str,
        _state: &[u8],
    ) -> Result<(), CoreError> {
        Ok(())
    }

    async fn load_checkpoint(
        &self,
        _instance_id: &str,
        _checkpoint_id: &str,
    ) -> Result<Option<CheckpointRecord>, CoreError> {
        Ok(None)
    }

    async fn list_checkpoints(
        &self,
        _instance_id: &str,
        _checkpoint_id: Option<&str>,
        _limit: i64,
        _offset: i64,
        _created_after: Option<DateTime<Utc>>,
        _created_before: Option<DateTime<Utc>>,
    ) -> Result<Vec<CheckpointRecord>, CoreError> {
        Ok(vec![])
    }

    async fn count_checkpoints(
        &self,
        _instance_id: &str,
        _checkpoint_id: Option<&str>,
        _created_after: Option<DateTime<Utc>>,
        _created_before: Option<DateTime<Utc>>,
    ) -> Result<i64, CoreError> {
        Ok(0)
    }

    async fn insert_event(&self, _event: &EventRecord) -> Result<(), CoreError> {
        Ok(())
    }

    async fn insert_signal(
        &self,
        _instance_id: &str,
        _signal_type: &str,
        _payload: &[u8],
    ) -> Result<(), CoreError> {
        Ok(())
    }

    async fn get_pending_signal(
        &self,
        _instance_id: &str,
    ) -> Result<Option<SignalRecord>, CoreError> {
        Ok(None)
    }

    async fn acknowledge_signal(&self, _instance_id: &str) -> Result<(), CoreError> {
        Ok(())
    }

    async fn insert_custom_signal(
        &self,
        _instance_id: &str,
        _checkpoint_id: &str,
        _payload: &[u8],
    ) -> Result<(), CoreError> {
        Ok(())
    }

    async fn take_pending_custom_signal(
        &self,
        _instance_id: &str,
        _checkpoint_id: &str,
    ) -> Result<Option<CustomSignalRecord>, CoreError> {
        Ok(None)
    }

    async fn save_retry_attempt(
        &self,
        _instance_id: &str,
        _checkpoint_id: &str,
        _attempt: i32,
        _error_message: Option<&str>,
    ) -> Result<(), CoreError> {
        Ok(())
    }

    async fn list_instances(
        &self,
        _tenant_id: Option<&str>,
        status: Option<&str>,
        _limit: i64,
        _offset: i64,
    ) -> Result<Vec<InstanceRecord>, CoreError> {
        let instances = self.instances.lock().unwrap();
        let filtered: Vec<InstanceRecord> = instances
            .values()
            .filter(|inst| status.is_none_or(|s| inst.status == s))
            .cloned()
            .collect();
        Ok(filtered)
    }

    async fn health_check_db(&self) -> Result<bool, CoreError> {
        Ok(true)
    }

    async fn count_active_instances(&self) -> Result<i64, CoreError> {
        Ok(self.instances.lock().unwrap().len() as i64)
    }

    async fn set_instance_sleep(
        &self,
        _instance_id: &str,
        _sleep_until: DateTime<Utc>,
    ) -> Result<(), CoreError> {
        Ok(())
    }

    async fn clear_instance_sleep(&self, _instance_id: &str) -> Result<(), CoreError> {
        Ok(())
    }

    async fn get_sleeping_instances_due(
        &self,
        _limit: i64,
    ) -> Result<Vec<InstanceRecord>, CoreError> {
        Ok(vec![])
    }

    async fn list_events(
        &self,
        _instance_id: &str,
        _filter: &ListEventsFilter,
        _limit: i64,
        _offset: i64,
    ) -> Result<Vec<EventRecord>, CoreError> {
        Ok(vec![])
    }

    async fn count_events(
        &self,
        _instance_id: &str,
        _filter: &ListEventsFilter,
    ) -> Result<i64, CoreError> {
        Ok(0)
    }

    async fn list_step_summaries(
        &self,
        _instance_id: &str,
        _filter: &ListStepSummariesFilter,
        _limit: i64,
        _offset: i64,
    ) -> Result<Vec<StepSummaryRecord>, CoreError> {
        Ok(vec![])
    }

    async fn count_step_summaries(
        &self,
        _instance_id: &str,
        _filter: &ListStepSummariesFilter,
    ) -> Result<i64, CoreError> {
        Ok(0)
    }
}

// ============================================================================
// HeartbeatMonitorConfig Tests (Unit tests - no DB required)
// ============================================================================

#[test]
fn test_heartbeat_monitor_config_default() {
    let config = HeartbeatMonitorConfig::default();
    assert_eq!(config.poll_interval, Duration::from_secs(30));
    assert_eq!(config.heartbeat_timeout, Duration::from_secs(120));
}

#[test]
fn test_heartbeat_monitor_config_custom() {
    let config = HeartbeatMonitorConfig {
        poll_interval: Duration::from_secs(60),
        heartbeat_timeout: Duration::from_secs(300),
    };
    assert_eq!(config.poll_interval, Duration::from_secs(60));
    assert_eq!(config.heartbeat_timeout, Duration::from_secs(300));
}

#[test]
fn test_heartbeat_monitor_config_clone() {
    let config = HeartbeatMonitorConfig {
        poll_interval: Duration::from_secs(15),
        heartbeat_timeout: Duration::from_secs(45),
    };

    let cloned = config.clone();
    assert_eq!(config.poll_interval, cloned.poll_interval);
    assert_eq!(config.heartbeat_timeout, cloned.heartbeat_timeout);
}

#[test]
fn test_heartbeat_monitor_config_debug() {
    let config = HeartbeatMonitorConfig::default();
    let debug_str = format!("{:?}", config);
    assert!(debug_str.contains("poll_interval"));
    assert!(debug_str.contains("heartbeat_timeout"));
}

// ============================================================================
// HeartbeatMonitor Creation Tests
// ============================================================================

#[tokio::test]
async fn test_heartbeat_monitor_creation() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let persistence = Arc::new(MockPersistence::new());
    let config = HeartbeatMonitorConfig::default();

    let monitor = HeartbeatMonitor::new(pool, persistence, config);
    let _shutdown = monitor.shutdown_handle();
    // Monitor created successfully
}

#[tokio::test]
async fn test_heartbeat_monitor_shutdown() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let persistence = Arc::new(MockPersistence::new());
    let config = HeartbeatMonitorConfig {
        poll_interval: Duration::from_millis(50),
        heartbeat_timeout: Duration::from_secs(120),
    };

    let monitor = HeartbeatMonitor::new(pool, persistence, config);
    let shutdown = monitor.shutdown_handle();

    // Start the monitor in a task
    let handle = tokio::spawn(async move {
        monitor.run().await;
    });

    // Give it a moment to start
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Signal shutdown
    shutdown.notify_one();

    // Wait for it to stop (with timeout)
    let result = tokio::time::timeout(Duration::from_secs(2), handle).await;
    assert!(result.is_ok(), "Monitor should shutdown promptly");
}

// ============================================================================
// Stale Container Detection Tests
// ============================================================================

#[tokio::test]
async fn test_stale_container_no_heartbeat() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let tenant_id = format!("test-tenant-stale-{}", Uuid::new_v4());
    let image_id = create_test_image(&pool, &tenant_id).await;
    let instance_id = Uuid::new_v4().to_string();

    // Create instance and register container but don't send heartbeat
    create_env_instance(&pool, &instance_id, &tenant_id, &image_id, "running").await;
    register_container(&pool, &instance_id, &tenant_id, &image_id).await;

    let persistence = Arc::new(MockPersistence::new());
    let config = HeartbeatMonitorConfig {
        poll_interval: Duration::from_millis(50),
        heartbeat_timeout: Duration::from_secs(60), // 1 minute timeout
    };

    let monitor = HeartbeatMonitor::new(pool.clone(), persistence.clone(), config);
    let shutdown = monitor.shutdown_handle();

    // Start monitor
    let handle = tokio::spawn(async move {
        monitor.run().await;
    });

    // Wait for at least one check cycle
    tokio::time::sleep(Duration::from_millis(150)).await;

    // Shutdown
    shutdown.notify_one();
    handle.await.ok();

    // The container should have been marked as failed (no heartbeat received)
    let completed = persistence.get_completed_instances();
    assert!(
        completed.iter().any(|(id, _, err)| {
            id == &instance_id && err.as_ref().is_some_and(|e| e.contains("stale"))
        }),
        "Instance should have been marked as stale due to missing heartbeat"
    );

    cleanup(&pool, &instance_id).await;
    cleanup_image(&pool, &image_id).await;
}

#[tokio::test]
async fn test_stale_container_old_heartbeat() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let tenant_id = format!("test-tenant-old-hb-{}", Uuid::new_v4());
    let image_id = create_test_image(&pool, &tenant_id).await;
    let instance_id = Uuid::new_v4().to_string();

    // Create instance, register container, and record old event in instance_events
    create_env_instance(&pool, &instance_id, &tenant_id, &image_id, "running").await;
    register_container(&pool, &instance_id, &tenant_id, &image_id).await;
    record_instance_event(&pool, &instance_id, &tenant_id, 10).await; // 10 minutes ago

    let persistence = Arc::new(MockPersistence::new());
    let config = HeartbeatMonitorConfig {
        poll_interval: Duration::from_millis(50),
        heartbeat_timeout: Duration::from_secs(120), // 2 minute timeout
    };

    let monitor = HeartbeatMonitor::new(pool.clone(), persistence.clone(), config);
    let shutdown = monitor.shutdown_handle();

    // Start monitor
    let handle = tokio::spawn(async move {
        monitor.run().await;
    });

    // Wait for at least one check cycle
    tokio::time::sleep(Duration::from_millis(150)).await;

    // Shutdown
    shutdown.notify_one();
    handle.await.ok();

    // The container should have been marked as failed (old activity)
    let completed = persistence.get_completed_instances();
    assert!(
        completed.iter().any(|(id, _, err)| {
            id == &instance_id && err.as_ref().is_some_and(|e| e.contains("stale"))
        }),
        "Instance should have been marked as stale due to old activity in instance_events"
    );

    cleanup(&pool, &instance_id).await;
    cleanup_image(&pool, &image_id).await;
}

#[tokio::test]
async fn test_container_with_recent_heartbeat_not_stale() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let tenant_id = format!("test-tenant-fresh-{}", Uuid::new_v4());
    let image_id = create_test_image(&pool, &tenant_id).await;
    let instance_id = Uuid::new_v4().to_string();

    // Create instance, register container, and record recent event in instance_events
    create_env_instance(&pool, &instance_id, &tenant_id, &image_id, "running").await;
    register_container(&pool, &instance_id, &tenant_id, &image_id).await;
    record_instance_event(&pool, &instance_id, &tenant_id, 0).await; // Just now

    let persistence = Arc::new(MockPersistence::new());
    let config = HeartbeatMonitorConfig {
        poll_interval: Duration::from_millis(50),
        heartbeat_timeout: Duration::from_secs(120), // 2 minute timeout
    };

    let monitor = HeartbeatMonitor::new(pool.clone(), persistence.clone(), config);
    let shutdown = monitor.shutdown_handle();

    // Start monitor
    let handle = tokio::spawn(async move {
        monitor.run().await;
    });

    // Wait for at least one check cycle
    tokio::time::sleep(Duration::from_millis(150)).await;

    // Shutdown
    shutdown.notify_one();
    handle.await.ok();

    // The container should NOT have been marked as failed (recent activity)
    let completed = persistence.get_completed_instances();
    assert!(
        !completed.iter().any(|(id, _, _)| id == &instance_id),
        "Instance with recent activity in instance_events should not be marked as stale"
    );

    cleanup(&pool, &instance_id).await;
    cleanup_image(&pool, &image_id).await;
}

// ============================================================================
// Orphaned Instance Detection Tests
// ============================================================================

#[tokio::test]
async fn test_orphaned_instance_detected() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let tenant_id = format!("test-tenant-orphan-{}", Uuid::new_v4());
    let instance_id = Uuid::new_v4().to_string();

    // Create instance as running in Core (mock persistence) but NOT in container_registry
    let started_at = Utc::now() - ChronoDuration::hours(1);
    let persistence = Arc::new(MockPersistence::new().with_running_instance(
        &instance_id,
        &tenant_id,
        started_at,
    ));

    let config = HeartbeatMonitorConfig {
        poll_interval: Duration::from_millis(50),
        heartbeat_timeout: Duration::from_secs(120), // 2 minute timeout
    };

    let monitor = HeartbeatMonitor::new(pool.clone(), persistence.clone(), config);
    let shutdown = monitor.shutdown_handle();

    // Start monitor
    let handle = tokio::spawn(async move {
        monitor.run().await;
    });

    // Wait for at least one check cycle
    tokio::time::sleep(Duration::from_millis(150)).await;

    // Shutdown
    shutdown.notify_one();
    handle.await.ok();

    // The orphaned instance should have been marked as failed
    let completed = persistence.get_completed_instances();
    assert!(
        completed.iter().any(|(id, _, err)| {
            id == &instance_id && err.as_ref().is_some_and(|e| e.contains("orphaned"))
        }),
        "Orphaned instance should have been marked as failed. Completed: {:?}",
        completed
    );
}

#[tokio::test]
async fn test_tracked_instance_not_orphaned() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let tenant_id = format!("test-tenant-tracked-{}", Uuid::new_v4());
    let image_id = create_test_image(&pool, &tenant_id).await;
    let instance_id = Uuid::new_v4().to_string();

    // Create instance in both Core persistence AND container_registry with recent activity
    let started_at = Utc::now() - ChronoDuration::hours(1);
    let persistence = Arc::new(MockPersistence::new().with_running_instance(
        &instance_id,
        &tenant_id,
        started_at,
    ));

    // Register in container_registry and record fresh activity in instance_events
    register_container(&pool, &instance_id, &tenant_id, &image_id).await;
    record_instance_event(&pool, &instance_id, &tenant_id, 0).await; // Fresh activity

    let config = HeartbeatMonitorConfig {
        poll_interval: Duration::from_millis(50),
        heartbeat_timeout: Duration::from_secs(120), // 2 minute timeout
    };

    let monitor = HeartbeatMonitor::new(pool.clone(), persistence.clone(), config);
    let shutdown = monitor.shutdown_handle();

    // Start monitor
    let handle = tokio::spawn(async move {
        monitor.run().await;
    });

    // Wait for at least one check cycle
    tokio::time::sleep(Duration::from_millis(150)).await;

    // Shutdown
    shutdown.notify_one();
    handle.await.ok();

    // The tracked instance should NOT have been marked as failed
    let completed = persistence.get_completed_instances();
    assert!(
        !completed.iter().any(|(id, _, _)| id == &instance_id),
        "Tracked instance with recent activity should not be marked as orphaned or stale"
    );

    cleanup(&pool, &instance_id).await;
    cleanup_image(&pool, &image_id).await;
}

#[tokio::test]
async fn test_recent_instance_not_immediately_orphaned() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let tenant_id = format!("test-tenant-recent-{}", Uuid::new_v4());
    let instance_id = Uuid::new_v4().to_string();

    // Create instance as running in Core but started very recently (within timeout)
    let started_at = Utc::now() - ChronoDuration::seconds(30); // 30 seconds ago
    let persistence = Arc::new(MockPersistence::new().with_running_instance(
        &instance_id,
        &tenant_id,
        started_at,
    ));

    let config = HeartbeatMonitorConfig {
        poll_interval: Duration::from_millis(50),
        heartbeat_timeout: Duration::from_secs(120), // 2 minute timeout
    };

    let monitor = HeartbeatMonitor::new(pool.clone(), persistence.clone(), config);
    let shutdown = monitor.shutdown_handle();

    // Start monitor
    let handle = tokio::spawn(async move {
        monitor.run().await;
    });

    // Wait for at least one check cycle
    tokio::time::sleep(Duration::from_millis(150)).await;

    // Shutdown
    shutdown.notify_one();
    handle.await.ok();

    // The recently started instance should NOT have been marked as failed (within grace period)
    let completed = persistence.get_completed_instances();
    assert!(
        !completed.iter().any(|(id, _, _)| id == &instance_id),
        "Recently started instance should not be immediately marked as orphaned"
    );
}

#[tokio::test]
async fn test_multiple_orphaned_instances() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let tenant_id = format!("test-tenant-multi-{}", Uuid::new_v4());
    let instance1 = Uuid::new_v4().to_string();
    let instance2 = Uuid::new_v4().to_string();
    let instance3 = Uuid::new_v4().to_string();

    // Create multiple orphaned instances at different times
    let old_start = Utc::now() - ChronoDuration::hours(2);
    let recent_start = Utc::now() - ChronoDuration::seconds(30);

    let persistence = Arc::new(
        MockPersistence::new()
            .with_running_instance(&instance1, &tenant_id, old_start)
            .with_running_instance(&instance2, &tenant_id, old_start)
            .with_running_instance(&instance3, &tenant_id, recent_start), // Should NOT be orphaned yet
    );

    let config = HeartbeatMonitorConfig {
        poll_interval: Duration::from_millis(50),
        heartbeat_timeout: Duration::from_secs(120), // 2 minute timeout
    };

    let monitor = HeartbeatMonitor::new(pool.clone(), persistence.clone(), config);
    let shutdown = monitor.shutdown_handle();

    // Start monitor
    let handle = tokio::spawn(async move {
        monitor.run().await;
    });

    // Wait for at least one check cycle
    tokio::time::sleep(Duration::from_millis(150)).await;

    // Shutdown
    shutdown.notify_one();
    handle.await.ok();

    // Check which instances were completed
    let completed = persistence.get_completed_instances();
    let completed_ids: Vec<&String> = completed.iter().map(|(id, _, _)| id).collect();

    // Old instances should be marked as orphaned
    assert!(
        completed_ids.contains(&&instance1),
        "Old instance 1 should be marked as orphaned"
    );
    assert!(
        completed_ids.contains(&&instance2),
        "Old instance 2 should be marked as orphaned"
    );

    // Recent instance should NOT be marked as orphaned
    assert!(
        !completed_ids.contains(&&instance3),
        "Recent instance 3 should not be immediately marked as orphaned"
    );
}

// ============================================================================
// Edge Cases
// ============================================================================

#[tokio::test]
async fn test_no_instances_to_check() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    // Empty persistence - no running instances
    let persistence = Arc::new(MockPersistence::new());

    let config = HeartbeatMonitorConfig {
        poll_interval: Duration::from_millis(50),
        heartbeat_timeout: Duration::from_secs(120),
    };

    let monitor = HeartbeatMonitor::new(pool.clone(), persistence.clone(), config);
    let shutdown = monitor.shutdown_handle();

    // Start monitor
    let handle = tokio::spawn(async move {
        monitor.run().await;
    });

    // Wait for at least one check cycle
    tokio::time::sleep(Duration::from_millis(150)).await;

    // Shutdown
    shutdown.notify_one();
    handle.await.ok();

    // No instances should have been completed
    let completed = persistence.get_completed_instances();
    assert!(
        completed.is_empty(),
        "No instances should be completed when there are none to check"
    );
}

#[tokio::test]
async fn test_completed_instance_in_core_not_flagged() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let tenant_id = format!("test-tenant-completed-{}", Uuid::new_v4());
    let instance_id = Uuid::new_v4().to_string();

    // Create an instance with "completed" status - should not be checked
    let persistence = Arc::new(MockPersistence::new());
    {
        let record = InstanceRecord {
            instance_id: instance_id.clone(),
            tenant_id: tenant_id.clone(),
            definition_version: 1,
            status: "completed".to_string(), // Not "running"
            checkpoint_id: None,
            attempt: 1,
            max_attempts: 3,
            created_at: Utc::now() - ChronoDuration::hours(2),
            started_at: Some(Utc::now() - ChronoDuration::hours(2)),
            finished_at: Some(Utc::now() - ChronoDuration::hours(1)),
            output: None,
            error: None,
            sleep_until: None,
            termination_reason: Some("completed".to_string()),
            exit_code: None,
        };
        persistence
            .instances
            .lock()
            .unwrap()
            .insert(instance_id.clone(), record);
    }

    let config = HeartbeatMonitorConfig {
        poll_interval: Duration::from_millis(50),
        heartbeat_timeout: Duration::from_secs(120),
    };

    let monitor = HeartbeatMonitor::new(pool.clone(), persistence.clone(), config);
    let shutdown = monitor.shutdown_handle();

    // Start monitor
    let handle = tokio::spawn(async move {
        monitor.run().await;
    });

    // Wait for at least one check cycle
    tokio::time::sleep(Duration::from_millis(150)).await;

    // Shutdown
    shutdown.notify_one();
    handle.await.ok();

    // Completed instance should NOT be re-completed
    let completed = persistence.get_completed_instances();
    assert!(
        !completed.iter().any(|(id, _, _)| id == &instance_id),
        "Already completed instance should not be flagged again"
    );
}

// ============================================================================
// Instance Events Based Activity Detection Tests
// ============================================================================

#[tokio::test]
async fn test_checkpoint_event_counts_as_activity() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let tenant_id = format!("test-tenant-checkpoint-{}", Uuid::new_v4());
    let image_id = create_test_image(&pool, &tenant_id).await;
    let instance_id = Uuid::new_v4().to_string();

    // Create instance, register container
    create_env_instance(&pool, &instance_id, &tenant_id, &image_id, "running").await;
    register_container(&pool, &instance_id, &tenant_id, &image_id).await;

    // Record a checkpoint event (simulating SDK checkpoint call)
    let event_id = Uuid::new_v4().to_string();
    sqlx::query(
        r#"
        INSERT INTO instance_events (event_id, instance_id, tenant_id, event_type, payload, created_at)
        VALUES ($1, $2, $3, 'checkpoint', $4, NOW())
        "#,
    )
    .bind(&event_id)
    .bind(&instance_id)
    .bind(&tenant_id)
    .bind(b"{}".as_slice())
    .execute(&pool)
    .await
    .expect("Failed to record checkpoint event");

    let persistence = Arc::new(MockPersistence::new());
    let config = HeartbeatMonitorConfig {
        poll_interval: Duration::from_millis(50),
        heartbeat_timeout: Duration::from_secs(120),
    };

    let monitor = HeartbeatMonitor::new(pool.clone(), persistence.clone(), config);
    let shutdown = monitor.shutdown_handle();

    let handle = tokio::spawn(async move {
        monitor.run().await;
    });

    tokio::time::sleep(Duration::from_millis(150)).await;
    shutdown.notify_one();
    handle.await.ok();

    // Instance should NOT be marked as stale (checkpoint event is recent activity)
    let completed = persistence.get_completed_instances();
    assert!(
        !completed.iter().any(|(id, _, _)| id == &instance_id),
        "Instance with recent checkpoint event should not be marked as stale"
    );

    cleanup(&pool, &instance_id).await;
    cleanup_image(&pool, &image_id).await;
}

#[tokio::test]
async fn test_any_event_type_counts_as_activity() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let tenant_id = format!("test-tenant-anyevent-{}", Uuid::new_v4());
    let image_id = create_test_image(&pool, &tenant_id).await;
    let instance_id = Uuid::new_v4().to_string();

    create_env_instance(&pool, &instance_id, &tenant_id, &image_id, "running").await;
    register_container(&pool, &instance_id, &tenant_id, &image_id).await;

    // Record a custom event type (not heartbeat or checkpoint)
    let event_id = Uuid::new_v4().to_string();
    sqlx::query(
        r#"
        INSERT INTO instance_events (event_id, instance_id, tenant_id, event_type, payload, created_at)
        VALUES ($1, $2, $3, 'custom_event', $4, NOW())
        "#,
    )
    .bind(&event_id)
    .bind(&instance_id)
    .bind(&tenant_id)
    .bind(b"{}".as_slice())
    .execute(&pool)
    .await
    .expect("Failed to record custom event");

    let persistence = Arc::new(MockPersistence::new());
    let config = HeartbeatMonitorConfig {
        poll_interval: Duration::from_millis(50),
        heartbeat_timeout: Duration::from_secs(120),
    };

    let monitor = HeartbeatMonitor::new(pool.clone(), persistence.clone(), config);
    let shutdown = monitor.shutdown_handle();

    let handle = tokio::spawn(async move {
        monitor.run().await;
    });

    tokio::time::sleep(Duration::from_millis(150)).await;
    shutdown.notify_one();
    handle.await.ok();

    // Any event type should count as activity
    let completed = persistence.get_completed_instances();
    assert!(
        !completed.iter().any(|(id, _, _)| id == &instance_id),
        "Instance with any recent event should not be marked as stale"
    );

    cleanup(&pool, &instance_id).await;
    cleanup_image(&pool, &image_id).await;
}

#[tokio::test]
async fn test_multiple_events_uses_most_recent() {
    skip_if_no_db!();
    let Some(pool) = get_test_pool().await else {
        eprintln!("Skipping test: could not connect to database");
        return;
    };

    let tenant_id = format!("test-tenant-multi-events-{}", Uuid::new_v4());
    let image_id = create_test_image(&pool, &tenant_id).await;
    let instance_id = Uuid::new_v4().to_string();

    create_env_instance(&pool, &instance_id, &tenant_id, &image_id, "running").await;
    register_container(&pool, &instance_id, &tenant_id, &image_id).await;

    // Record an old event (10 minutes ago)
    record_instance_event(&pool, &instance_id, &tenant_id, 10).await;

    // Record a recent event (just now)
    record_instance_event(&pool, &instance_id, &tenant_id, 0).await;

    let persistence = Arc::new(MockPersistence::new());
    let config = HeartbeatMonitorConfig {
        poll_interval: Duration::from_millis(50),
        heartbeat_timeout: Duration::from_secs(120), // 2 minute timeout
    };

    let monitor = HeartbeatMonitor::new(pool.clone(), persistence.clone(), config);
    let shutdown = monitor.shutdown_handle();

    let handle = tokio::spawn(async move {
        monitor.run().await;
    });

    tokio::time::sleep(Duration::from_millis(150)).await;
    shutdown.notify_one();
    handle.await.ok();

    // Should use the most recent event, so instance should be alive
    let completed = persistence.get_completed_instances();
    assert!(
        !completed.iter().any(|(id, _, _)| id == &instance_id),
        "Instance with recent event among multiple should not be marked as stale"
    );

    cleanup(&pool, &instance_id).await;
    cleanup_image(&pool, &image_id).await;
}
