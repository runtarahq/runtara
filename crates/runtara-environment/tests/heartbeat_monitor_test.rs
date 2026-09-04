// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Tests for heartbeat_monitor module - detecting and failing stale/orphaned instances.

mod common;

use async_trait::async_trait;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use runtara_core::error::CoreError;
use runtara_core::persistence::{
    CheckpointRecord, CompleteInstanceParams, CustomSignalRecord, EventRecord, EventVocabulary,
    InstanceRecord, ListEventsFilter, ListPairedRecordsFilter, PairedRecordSummary, Persistence,
    SignalRecord,
};
use runtara_environment::container_registry::ContainerRegistry;
use runtara_environment::heartbeat_monitor::{HeartbeatMonitor, HeartbeatMonitorConfig};
use runtara_environment::runner::{
    CancelToken, ContainerMetrics, LaunchOptions, LaunchResult, Runner, RunnerHandle,
};
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use uuid::Uuid;

// ============================================================================
// Mock Runner for Testing
// ============================================================================

/// No-op runner that does nothing. Used to satisfy HeartbeatMonitor's runner parameter.
struct MockRunner;

#[async_trait]
impl Runner for MockRunner {
    fn runner_type(&self) -> &'static str {
        "mock"
    }

    async fn run(
        &self,
        _options: &LaunchOptions,
        _cancel_token: Option<CancelToken>,
    ) -> runtara_environment::runner::Result<LaunchResult> {
        unimplemented!("MockRunner::run not needed for heartbeat monitor tests")
    }

    async fn launch_detached(
        &self,
        _options: &LaunchOptions,
    ) -> runtara_environment::runner::Result<RunnerHandle> {
        unimplemented!("MockRunner::launch_detached not needed for heartbeat monitor tests")
    }

    async fn try_launch_detached(
        &self,
        options: &LaunchOptions,
    ) -> runtara_environment::runner::Result<RunnerHandle> {
        self.launch_detached(options).await
    }

    async fn is_running(&self, _handle: &RunnerHandle) -> bool {
        false
    }

    async fn stop(&self, _handle: &RunnerHandle) -> runtara_environment::runner::Result<()> {
        Ok(())
    }

    async fn collect_result(
        &self,
        _handle: &RunnerHandle,
    ) -> (Option<serde_json::Value>, Option<String>, ContainerMetrics) {
        (None, None, ContainerMetrics::default())
    }
}

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

/// Create a test image in the database with a unique name
async fn create_test_image(pool: &PgPool, tenant_id: &str) -> String {
    let image_id = Uuid::new_v4().to_string();
    let image_name = format!("test-image-{}", image_id);
    sqlx::query(
        r#"
        INSERT INTO images (image_id, tenant_id, name, description, binary_path)
        VALUES ($1, $2, $3, 'Test image', '/usr/bin/test')
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
    _image_id: &str,
    status: &str,
) {
    sqlx::query(
        r#"
        INSERT INTO instances (instance_id, tenant_id, status, created_at, started_at)
        VALUES ($1, $2, $3::instance_status, NOW() - INTERVAL '1 hour', NOW() - INTERVAL '1 hour')
        "#,
    )
    .bind(instance_id)
    .bind(tenant_id)
    .bind(status)
    .execute(pool)
    .await
    .expect("Failed to create test instance");
}

/// Register a container in container_registry
async fn register_container(pool: &PgPool, instance_id: &str, tenant_id: &str, _image_id: &str) {
    let container_id = format!("runtara_{}", &instance_id[..8.min(instance_id.len())]);
    sqlx::query(
        r#"
        INSERT INTO container_registry (container_id, launch_id, instance_id, tenant_id, binary_path, started_at)
        VALUES ($1, $1, $2, $3, '/usr/bin/test', NOW() - INTERVAL '30 minutes')
        "#,
    )
    .bind(&container_id)
    .bind(instance_id)
    .bind(tenant_id)
    .execute(pool)
    .await
    .expect("Failed to register container");
}

/// Record an event in instance_events table (used by HeartbeatMonitor for activity detection)
async fn record_instance_event(
    pool: &PgPool,
    instance_id: &str,
    _tenant_id: &str,
    minutes_ago: i64,
) {
    let event_time = Utc::now() - ChronoDuration::minutes(minutes_ago);
    sqlx::query(
        r#"
        INSERT INTO instance_events (instance_id, event_type, payload, created_at)
        VALUES ($1, 'heartbeat', $2, $3)
        "#,
    )
    .bind(instance_id)
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
            input: None,
            output: None,
            error: None,
            sleep_until: None,
            termination_reason: None,
            exit_code: None,
            recovery_attempts: 0,
            recovery_marker: None,
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
        params: CompleteInstanceParams<'_>,
    ) -> Result<bool, CoreError> {
        self.completed_instances.lock().unwrap().push((
            params.instance_id.to_string(),
            params.output.map(|o| o.to_vec()),
            params.error.map(|e| e.to_string()),
        ));
        // Remove from instances
        self.instances.lock().unwrap().remove(params.instance_id);
        Ok(true)
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

    async fn list_paired_records(
        &self,
        _instance_id: &str,
        _vocabulary: &EventVocabulary,
        _filter: &ListPairedRecordsFilter,
        _limit: i64,
        _offset: i64,
    ) -> Result<Vec<PairedRecordSummary>, CoreError> {
        Ok(vec![])
    }

    async fn count_paired_records(
        &self,
        _instance_id: &str,
        _vocabulary: &EventVocabulary,
        _filter: &ListPairedRecordsFilter,
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
// HeartbeatMonitor Lifecycle Tests
// ============================================================================

#[tokio::test]
async fn test_heartbeat_monitor_shutdown() {
    skip_if_no_db!();
    let pool = get_test_pool().await;

    let persistence = Arc::new(MockPersistence::new());
    let config = HeartbeatMonitorConfig {
        poll_interval: Duration::from_millis(50),
        heartbeat_timeout: Duration::from_secs(120),
    };

    let monitor = HeartbeatMonitor::new(pool, persistence, Arc::new(MockRunner), config);
    let shutdown = monitor.shutdown_handle();

    // Start the monitor in a task
    let handle = tokio::spawn(async move {
        monitor.run().await;
    });

    // Give it a moment to start, so shutdown races against a running poll loop
    // rather than arriving before `run` has begun.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Signal shutdown
    shutdown.notify_one();

    // Wait for it to stop (with timeout). Both layers matter: the outer one is
    // "did it stop in time", the inner one is "did it stop cleanly" — a panic
    // inside `run` also joins promptly and would otherwise pass unnoticed.
    tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .expect("monitor should shut down within 2s of being notified")
        .expect("monitor task should exit cleanly, not panic");
}

// ============================================================================
// Stale Container Detection Tests
// ============================================================================

#[tokio::test]
async fn test_stale_container_no_heartbeat() {
    skip_if_no_db!();
    let pool = get_test_pool().await;

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

    let monitor = HeartbeatMonitor::new(
        pool.clone(),
        persistence.clone(),
        Arc::new(MockRunner),
        config,
    );
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
    let pool = get_test_pool().await;

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

    let monitor = HeartbeatMonitor::new(
        pool.clone(),
        persistence.clone(),
        Arc::new(MockRunner),
        config,
    );
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
    let pool = get_test_pool().await;

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

    let monitor = HeartbeatMonitor::new(
        pool.clone(),
        persistence.clone(),
        Arc::new(MockRunner),
        config,
    );
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
    let pool = get_test_pool().await;

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

    let monitor = HeartbeatMonitor::new(
        pool.clone(),
        persistence.clone(),
        Arc::new(MockRunner),
        config,
    );
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

    // The orphaned instance should have entered the default automatic-recovery
    // path. The mock's default mark_for_recovery implementation records the
    // suspended completion without an error.
    let completed = persistence.get_completed_instances();
    assert!(
        completed
            .iter()
            .any(|(id, _, err)| id == &instance_id && err.is_none()),
        "Orphaned instance should have been marked for recovery. Completed: {:?}",
        completed
    );
}

#[tokio::test]
async fn test_tracked_instance_not_orphaned() {
    skip_if_no_db!();
    let pool = get_test_pool().await;

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
    create_env_instance(&pool, &instance_id, &tenant_id, &image_id, "running").await;
    register_container(&pool, &instance_id, &tenant_id, &image_id).await;
    record_instance_event(&pool, &instance_id, &tenant_id, 0).await; // Fresh activity

    let config = HeartbeatMonitorConfig {
        poll_interval: Duration::from_millis(50),
        heartbeat_timeout: Duration::from_secs(120), // 2 minute timeout
    };

    let monitor = HeartbeatMonitor::new(
        pool.clone(),
        persistence.clone(),
        Arc::new(MockRunner),
        config,
    );
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
    let pool = get_test_pool().await;

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

    let monitor = HeartbeatMonitor::new(
        pool.clone(),
        persistence.clone(),
        Arc::new(MockRunner),
        config,
    );
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
    let pool = get_test_pool().await;

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

    let monitor = HeartbeatMonitor::new(
        pool.clone(),
        persistence.clone(),
        Arc::new(MockRunner),
        config,
    );
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
    let pool = get_test_pool().await;

    // Empty persistence - no running instances
    let persistence = Arc::new(MockPersistence::new());

    let config = HeartbeatMonitorConfig {
        poll_interval: Duration::from_millis(50),
        heartbeat_timeout: Duration::from_secs(120),
    };

    let monitor = HeartbeatMonitor::new(
        pool.clone(),
        persistence.clone(),
        Arc::new(MockRunner),
        config,
    );
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
    let pool = get_test_pool().await;

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
            input: None,
            output: None,
            error: None,
            sleep_until: None,
            termination_reason: Some("completed".to_string()),
            exit_code: None,
            recovery_attempts: 0,
            recovery_marker: None,
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

    let monitor = HeartbeatMonitor::new(
        pool.clone(),
        persistence.clone(),
        Arc::new(MockRunner),
        config,
    );
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
    let pool = get_test_pool().await;

    let tenant_id = format!("test-tenant-checkpoint-{}", Uuid::new_v4());
    let image_id = create_test_image(&pool, &tenant_id).await;
    let instance_id = Uuid::new_v4().to_string();

    // Create instance, register container
    create_env_instance(&pool, &instance_id, &tenant_id, &image_id, "running").await;
    register_container(&pool, &instance_id, &tenant_id, &image_id).await;

    // Record a progress event (the current schema representation of durable
    // execution progress such as a checkpoint).
    sqlx::query(
        r#"
        INSERT INTO instance_events (instance_id, event_type, payload, created_at)
        VALUES ($1, 'progress', $2, NOW())
        "#,
    )
    .bind(&instance_id)
    .bind(b"{}".as_slice())
    .execute(&pool)
    .await
    .expect("Failed to record checkpoint event");

    let persistence = Arc::new(MockPersistence::new());
    let config = HeartbeatMonitorConfig {
        poll_interval: Duration::from_millis(50),
        heartbeat_timeout: Duration::from_secs(120),
    };

    let monitor = HeartbeatMonitor::new(
        pool.clone(),
        persistence.clone(),
        Arc::new(MockRunner),
        config,
    );
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
    let pool = get_test_pool().await;

    let tenant_id = format!("test-tenant-anyevent-{}", Uuid::new_v4());
    let image_id = create_test_image(&pool, &tenant_id).await;
    let instance_id = Uuid::new_v4().to_string();

    create_env_instance(&pool, &instance_id, &tenant_id, &image_id, "running").await;
    register_container(&pool, &instance_id, &tenant_id, &image_id).await;

    // Record a custom event type (not heartbeat or progress).
    sqlx::query(
        r#"
        INSERT INTO instance_events (instance_id, event_type, payload, created_at)
        VALUES ($1, 'custom', $2, NOW())
        "#,
    )
    .bind(&instance_id)
    .bind(b"{}".as_slice())
    .execute(&pool)
    .await
    .expect("Failed to record custom event");

    let persistence = Arc::new(MockPersistence::new());
    let config = HeartbeatMonitorConfig {
        poll_interval: Duration::from_millis(50),
        heartbeat_timeout: Duration::from_secs(120),
    };

    let monitor = HeartbeatMonitor::new(
        pool.clone(),
        persistence.clone(),
        Arc::new(MockRunner),
        config,
    );
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
    let pool = get_test_pool().await;

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

    let monitor = HeartbeatMonitor::new(
        pool.clone(),
        persistence.clone(),
        Arc::new(MockRunner),
        config,
    );
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

/// A just-woken sleeper must not be failed for the events it wrote before it slept.
///
/// Regression: the staleness predicate had two branches, and only the
/// "no events at all" one required the container to have been running longer
/// than the timeout. The "last event is old" branch looked at
/// `MAX(instance_events.created_at)` alone — which for a woken sleeper is the
/// event it wrote *before* going to sleep, arbitrarily long ago — so every wake
/// looked stale between registering its container and the relaunched guest
/// writing its first event. The instance genuinely is `running` in that window,
/// so the `if_running` guard on the failing write does not save it, and a run
/// that goes on to succeed is recorded as a heartbeat timeout instead.
#[tokio::test]
async fn test_freshly_woken_instance_is_not_stale_despite_old_events() {
    skip_if_no_db!();
    let pool = get_test_pool().await;

    let tenant_id = format!("test-tenant-woken-{}", Uuid::new_v4());
    let image_id = create_test_image(&pool, &tenant_id).await;
    let instance_id = Uuid::new_v4().to_string();

    create_env_instance(&pool, &instance_id, &tenant_id, &image_id, "running").await;

    // The shape of a wake: the container was registered moments ago, but the
    // newest event predates the sleep by hours.
    let container_id = format!("runtara_{}", &instance_id[..8]);
    sqlx::query(
        r#"
        INSERT INTO container_registry (container_id, launch_id, instance_id, tenant_id, binary_path, started_at)
        VALUES ($1, $1, $2, $3, '/usr/bin/test', NOW())
        "#,
    )
    .bind(&container_id)
    .bind(&instance_id)
    .bind(&tenant_id)
    .execute(&pool)
    .await
    .expect("Failed to register container");

    record_instance_event(&pool, &instance_id, &tenant_id, 6 * 60).await;

    let persistence = Arc::new(MockPersistence::new());
    let config = HeartbeatMonitorConfig {
        poll_interval: Duration::from_millis(50),
        heartbeat_timeout: Duration::from_secs(120),
    };

    let monitor = HeartbeatMonitor::new(
        pool.clone(),
        persistence.clone(),
        Arc::new(MockRunner),
        config,
    );
    let shutdown = monitor.shutdown_handle();
    let handle = tokio::spawn(async move {
        monitor.run().await;
    });

    tokio::time::sleep(Duration::from_millis(150)).await;
    shutdown.notify_one();
    handle.await.ok();

    let completed = persistence.get_completed_instances();
    assert!(
        !completed.iter().any(|(id, _, _)| id == &instance_id),
        "an instance whose container started just now must not be failed for \
         events written before it went to sleep"
    );

    cleanup(&pool, &instance_id).await;
    cleanup_image(&pool, &image_id).await;
}

/// The generation guard the stale-instance failure path depends on.
///
/// Anything that selects a container and acts on it later is racing a wake: the
/// instance can be relaunched in between, replacing the registry row with a new
/// `container_id`. Deleting by instance alone would throw away the live run's
/// row, and `if_running` would then be satisfied by that live run, so the
/// monitor would mark a healthy instance failed. The guarded delete is what
/// makes the failure path notice.
#[tokio::test]
async fn cleanup_generation_refuses_to_remove_a_replacement_container() {
    skip_if_no_db!();
    let pool = get_test_pool().await;
    let tenant_id = format!("test-tenant-generation-{}", Uuid::new_v4());
    let image_id = create_test_image(&pool, &tenant_id).await;
    let instance_id = Uuid::new_v4().to_string();
    create_env_instance(&pool, &instance_id, &tenant_id, &image_id, "running").await;

    let registry = ContainerRegistry::new(pool.clone());
    let insert = |container_id: String| {
        let pool = pool.clone();
        let instance_id = instance_id.clone();
        let tenant_id = tenant_id.clone();
        async move {
            sqlx::query(
                r#"
                INSERT INTO container_registry
                    (container_id, launch_id, instance_id, tenant_id, binary_path, started_at)
                VALUES ($1, $1, $2, $3, '/usr/bin/test', NOW())
                "#,
            )
            .bind(container_id)
            .bind(&instance_id)
            .bind(&tenant_id)
            .execute(&pool)
            .await
            .expect("Failed to register container");
        }
    };

    // The generation a scan would have selected.
    insert("gen-old".to_string()).await;

    // The instance is woken and relaunched: the old row goes, a new one arrives.
    registry
        .cleanup(&instance_id)
        .await
        .expect("cleanup failed");
    insert("gen-new".to_string()).await;

    // The scan's stale processing now arrives, still holding the old id.
    let removed = registry
        .cleanup_generation(&instance_id, "gen-old")
        .await
        .expect("cleanup_generation failed");
    assert!(
        !removed,
        "a stale scan must not claim an instance that has been relaunched"
    );
    assert_eq!(
        registry
            .get(&instance_id)
            .await
            .expect("registry read failed")
            .expect("the replacement container must still be registered")
            .container_id,
        "gen-new",
        "the live run's registry row must survive a stale scan"
    );

    // And the guard still lets the owning generation through.
    assert!(
        registry
            .cleanup_generation(&instance_id, "gen-new")
            .await
            .expect("cleanup_generation failed"),
        "the current generation must be able to claim its own row"
    );

    cleanup(&pool, &instance_id).await;
    cleanup_image(&pool, &image_id).await;
}

/// The fix must not stop the monitor catching a genuinely hung instance: old
/// container, old events, nothing recent.
#[tokio::test]
async fn test_long_running_instance_with_old_events_is_still_stale() {
    skip_if_no_db!();
    let pool = get_test_pool().await;

    let tenant_id = format!("test-tenant-hung-{}", Uuid::new_v4());
    let image_id = create_test_image(&pool, &tenant_id).await;
    let instance_id = Uuid::new_v4().to_string();

    create_env_instance(&pool, &instance_id, &tenant_id, &image_id, "running").await;
    // register_container backdates started_at by 30 minutes.
    register_container(&pool, &instance_id, &tenant_id, &image_id).await;
    record_instance_event(&pool, &instance_id, &tenant_id, 20).await;

    let persistence = Arc::new(MockPersistence::new());
    let config = HeartbeatMonitorConfig {
        poll_interval: Duration::from_millis(50),
        heartbeat_timeout: Duration::from_secs(120),
    };

    let monitor = HeartbeatMonitor::new(
        pool.clone(),
        persistence.clone(),
        Arc::new(MockRunner),
        config,
    );
    let shutdown = monitor.shutdown_handle();
    let handle = tokio::spawn(async move {
        monitor.run().await;
    });

    tokio::time::sleep(Duration::from_millis(150)).await;
    shutdown.notify_one();
    handle.await.ok();

    let completed = persistence.get_completed_instances();
    assert!(
        completed.iter().any(|(id, _, error)| {
            id == &instance_id
                && error
                    .as_deref()
                    .is_some_and(|e| e.contains("Instance stale"))
        }),
        "an instance running for 30 minutes with no activity for 20 must still be failed"
    );

    cleanup(&pool, &instance_id).await;
    cleanup_image(&pool, &image_id).await;
}
