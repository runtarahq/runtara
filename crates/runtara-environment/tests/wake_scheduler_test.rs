// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Tests for wake_scheduler module and related database operations.

mod common;

use chrono::Utc;
use runtara_core::persistence::{CompleteInstanceParams, Persistence, PostgresPersistence};
use runtara_environment::container_registry::ContainerRegistry;
use runtara_environment::db::{self, Instance};
use runtara_environment::runner::{MockRunner, Runner};
use runtara_environment::wake_scheduler::{WakeScheduler, WakeSchedulerConfig};
use sqlx::PgPool;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
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

/// Helper to create a test instance using the Persistence trait.
/// This replaces the old `db::create_instance` function that was removed.
async fn create_test_instance(pool: &PgPool, instance_id: &str, tenant_id: &str, image_id: &str) {
    let persistence = PostgresPersistence::new(pool.clone());
    persistence
        .register_instance(instance_id, tenant_id)
        .await
        .expect("Failed to register instance");
    db::associate_instance_image(pool, instance_id, image_id, tenant_id, None, None)
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
    if matches!(status, "completed" | "failed" | "cancelled") {
        let mut params = CompleteInstanceParams::new(instance_id, status);
        if let Some(cp_id) = checkpoint_id {
            params = params.with_checkpoint(cp_id);
        }
        persistence
            .complete_instance(params)
            .await
            .expect("Failed to complete instance");
        return;
    }

    let started_at = (status == "running").then(Utc::now);
    persistence
        .update_instance_status(instance_id, status, started_at)
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
    let mut params = CompleteInstanceParams::new(instance_id, status);
    if let Some(o) = output {
        params = params.with_output(o);
    }
    if let Some(e) = error {
        params = params.with_error(e);
    }
    if let Some(s) = stderr {
        params = params.with_stderr(s);
    }
    if let Some(cp) = checkpoint_id {
        params = params.with_checkpoint(cp);
    }
    persistence
        .complete_instance(params)
        .await
        .expect("Failed to update instance result");
}

// ============================================================================
// WakeSchedulerConfig Tests (Unit tests - no DB required)
// ============================================================================

#[test]
fn test_wake_scheduler_config_default() {
    let config = WakeSchedulerConfig::default();
    // `poll_interval` is the idle wait, not a rate limit: a poll that fills its
    // batch is followed immediately by the next one.
    assert_eq!(config.poll_interval, Duration::from_secs(5));
    // A batch of 10 could not feed a concurrent waker.
    assert_eq!(config.batch_size, 200);
    assert!(
        (1..=512).contains(&config.concurrency),
        "in-batch concurrency must be bounded and non-zero: {}",
        config.concurrency
    );
    assert_eq!(config.core_addr, "127.0.0.1:8001");
    assert_eq!(config.data_dir, PathBuf::from(".data"));
}

#[test]
fn test_wake_scheduler_config_custom() {
    let config = WakeSchedulerConfig {
        poll_interval: Duration::from_secs(10),
        batch_size: 50,
        concurrency: 4,
        claim_lease: Duration::from_secs(300),
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
        concurrency: 4,
        claim_lease: Duration::from_secs(300),
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
    let pool = get_test_pool().await;

    let instance_id = Uuid::new_v4().to_string();
    let tenant_id = "test-tenant";
    let image_id = create_test_image(&pool, tenant_id).await;

    create_test_instance(&pool, &instance_id, tenant_id, &image_id).await;

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
    let pool = get_test_pool().await;

    let instance_id = Uuid::new_v4().to_string();
    let tenant_id = "test-tenant";
    let image_id = create_test_image(&pool, tenant_id).await;

    create_test_instance(&pool, &instance_id, tenant_id, &image_id).await;

    // Update to running
    update_test_instance_status(&pool, &instance_id, "running", None).await;

    let instance = db::get_instance(&pool, &instance_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(instance.status, "running");
    assert!(instance.started_at.is_some()); // Should be set when status = running

    // Update to completed
    update_test_instance_status(&pool, &instance_id, "completed", Some("cp-final")).await;

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
    let pool = get_test_pool().await;

    let instance_id = Uuid::new_v4().to_string();
    let tenant_id = "test-tenant";
    let image_id = create_test_image(&pool, tenant_id).await;

    create_test_instance(&pool, &instance_id, tenant_id, &image_id).await;

    let output = serde_json::json!({"result": "success"});
    let output_bytes = serde_json::to_vec(&output).unwrap();

    update_test_instance_result(
        &pool,
        &instance_id,
        "completed",
        Some(&output_bytes),
        None,
        Some("cp-done"),
        None, // stderr
    )
    .await;

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
    let pool = get_test_pool().await;

    let instance_id = Uuid::new_v4().to_string();
    let tenant_id = "test-tenant";
    let image_id = create_test_image(&pool, tenant_id).await;

    create_test_instance(&pool, &instance_id, tenant_id, &image_id).await;

    update_test_instance_result(
        &pool,
        &instance_id,
        "failed",
        None,
        Some("Connection refused"),
        None,
        Some("error: could not connect to server"), // stderr
    )
    .await;

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
    let pool = get_test_pool().await;

    // Clean up first
    sqlx::query("DELETE FROM instances WHERE tenant_id LIKE 'list-test-%'")
        .execute(&pool)
        .await
        .ok();

    let image_id = create_test_image(&pool, "list-test-tenant-a").await;

    let instance1 = Uuid::new_v4().to_string();
    let instance2 = Uuid::new_v4().to_string();
    let instance3 = Uuid::new_v4().to_string();

    create_test_instance(&pool, &instance1, "list-test-tenant-a", &image_id).await;
    create_test_instance(&pool, &instance2, "list-test-tenant-a", &image_id).await;
    create_test_instance(&pool, &instance3, "list-test-tenant-b", &image_id).await;

    // Update statuses
    update_test_instance_status(&pool, &instance1, "running", None).await;
    update_test_instance_status(&pool, &instance2, "completed", None).await;

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
        statuses: Some(vec!["running".to_string()]),
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

    // List with offset. Scoped to this test's own tenant: unscoped, both pages
    // saturate at `limit` as soon as the shared database holds more than 100
    // instances, and the assertion silently compares 100 against 100 - 1.
    let all_options = db::ListInstancesOptions {
        tenant_id: Some("list-test-tenant-a".to_string()),
        limit: 100,
        ..Default::default()
    };
    let all = db::list_instances(&pool, &all_options).await.unwrap();
    let offset_options = db::ListInstancesOptions {
        tenant_id: Some("list-test-tenant-a".to_string()),
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
    let pool = get_test_pool().await;

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
        concurrency: 4,
        claim_lease: Duration::from_secs(300),
        core_addr: "127.0.0.1:8001".to_string(),
        data_dir: PathBuf::from("/custom/data/dir"),
    };

    assert_eq!(config.data_dir, PathBuf::from("/custom/data/dir"));
}

// ============================================================================
// Cancel-at-wake (SYN-606)
// ============================================================================

/// Park an instance as a durable sleep leaves it: suspended, with a wake time
/// already past. Returns its id.
async fn park_due_instance(pool: &PgPool, tenant_id: &str, image_id: &str) -> String {
    let instance_id = Uuid::new_v4().to_string();
    create_test_instance(pool, &instance_id, tenant_id, image_id).await;
    update_test_instance_status(pool, &instance_id, "suspended", Some("delay-1")).await;
    PostgresPersistence::new(pool.clone())
        .set_instance_sleep(&instance_id, Utc::now() - chrono::Duration::seconds(1))
        .await
        .expect("Failed to stamp sleep_until");
    instance_id
}

/// A cancel that arrives while an instance sleeps has nobody to observe it: the
/// guest is not running, and a relaunch replays into a checkpoint HIT that skips
/// the guest's poll sites. The scheduler must resolve it directly — no launch,
/// terminal status `cancelled` — rather than starting a process that will ignore
/// the signal and run to completion.
///
/// Both cases run under ONE scheduler on purpose. Every suspended-and-due row is
/// a candidate for any scheduler polling this database, so two schedulers racing
/// in separate tests would each pick up the other's instance.
#[tokio::test]
async fn test_wake_cancels_pending_cancel_and_still_launches_the_rest() {
    skip_if_no_db!();
    let pool = get_test_pool().await;

    let tenant_id = "test-tenant-syn606";
    let image_id = create_test_image(&pool, tenant_id).await;
    let cancelled_id = park_due_instance(&pool, tenant_id, &image_id).await;
    let healthy_id = park_due_instance(&pool, tenant_id, &image_id).await;

    let persistence = Arc::new(PostgresPersistence::new(pool.clone()));
    persistence
        .insert_signal(&cancelled_id, "cancel", b"")
        .await
        .expect("Failed to insert cancel signal");

    // `never_completing` keeps the launched instance's registry row in place;
    // a self-completing mock would unregister it out from under the assertion.
    let runner: Arc<dyn Runner> = Arc::new(MockRunner::never_completing());
    let scheduler = WakeScheduler::new(
        pool.clone(),
        persistence.clone(),
        Arc::clone(&runner),
        WakeSchedulerConfig {
            poll_interval: Duration::from_millis(100),
            batch_size: 10,
            concurrency: 4,
            claim_lease: Duration::from_secs(300),
            core_addr: "127.0.0.1:8001".to_string(),
            data_dir: PathBuf::from(".data"),
        },
    );
    let shutdown = scheduler.shutdown_handle();
    let handle = tokio::spawn(scheduler.run());

    // Launch is instance-scoped: waking registers a container row for that id.
    let registry = ContainerRegistry::new(pool.clone());
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let mut status = String::new();
    let mut healthy_launched = false;
    while std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
        status = persistence
            .get_instance(&cancelled_id)
            .await
            .expect("Failed to read instance")
            .expect("Instance should exist")
            .status;
        healthy_launched = registry
            .get(&healthy_id)
            .await
            .expect("Failed to read container registry")
            .is_some();
        if status == "cancelled" && healthy_launched {
            break;
        }
    }

    shutdown.notify_one();
    let _ = handle.await;

    assert_eq!(
        status, "cancelled",
        "a cancel pending at wake time must land the instance on `cancelled`"
    );
    assert!(
        registry
            .get(&cancelled_id)
            .await
            .expect("Failed to read container registry")
            .is_none(),
        "the cancelled instance must not be relaunched just to be cancelled"
    );
    assert!(
        healthy_launched,
        "a due instance with no pending cancel must still be relaunched"
    );

    cleanup(&pool, &cancelled_id).await;
    cleanup(&pool, &healthy_id).await;
    cleanup_image(&pool, &image_id).await;
}

// ============================================================================
// Claim-release and in-batch concurrency
// ============================================================================

/// A launch failure must put the instance back in the wake candidate set.
///
/// The batch claim clears `sleep_until` as it selects, so from that moment the
/// wake scan can no longer see the instance. If the relaunch then fails and
/// nothing restores the deadline, the instance is stranded `suspended` forever
/// — it will never be picked up again. The scheduler re-stamps it instead.
#[tokio::test]
async fn a_failed_wake_returns_the_instance_to_the_candidate_set() {
    skip_if_no_db!();
    let pool = get_test_pool().await;
    let tenant_id = "wake-failure-tenant";
    let image_id = create_test_image(&pool, tenant_id).await;
    let instance_id = park_due_instance(&pool, tenant_id, &image_id).await;

    let persistence = Arc::new(PostgresPersistence::new(pool.clone()));
    let scheduler = WakeScheduler::new(
        pool.clone(),
        persistence.clone(),
        Arc::new(MockRunner::failing()),
        WakeSchedulerConfig {
            poll_interval: Duration::from_millis(50),
            batch_size: 10,
            concurrency: 4,
            claim_lease: Duration::from_secs(300),
            core_addr: "127.0.0.1:8001".to_string(),
            data_dir: PathBuf::from(".data"),
        },
    );
    let shutdown = scheduler.shutdown_handle();
    let handle = tokio::spawn(scheduler.run());

    // Give the scheduler time to claim, fail the launch, and restore.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let mut restored = false;
    while std::time::Instant::now() < deadline {
        let inst = persistence
            .get_instance(&instance_id)
            .await
            .unwrap()
            .expect("instance must exist");
        if inst.status == "suspended" && inst.sleep_until.is_some() {
            restored = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    shutdown.notify_one();
    let _ = handle.await;

    assert!(
        restored,
        "a wake whose launch failed must leave the instance suspended with a \
         deadline, so a later poll retries it instead of stranding it"
    );

    cleanup(&pool, &instance_id).await;
    cleanup_image(&pool, &image_id).await;
}

/// A runner that holds each launch open briefly and records how many were in
/// flight at once, so the test can see whether the batch is actually spread
/// across tasks or awaited one at a time.
struct ConcurrencyProbeRunner {
    in_flight: Arc<std::sync::atomic::AtomicUsize>,
    peak: Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait::async_trait]
impl Runner for ConcurrencyProbeRunner {
    fn runner_type(&self) -> &'static str {
        "concurrency-probe"
    }

    async fn run(
        &self,
        _options: &runtara_environment::runner::LaunchOptions,
        _cancel: Option<runtara_environment::runner::CancelToken>,
    ) -> runtara_environment::runner::Result<runtara_environment::runner::LaunchResult> {
        unimplemented!("the wake path uses launch_detached")
    }

    async fn launch_detached(
        &self,
        options: &runtara_environment::runner::LaunchOptions,
    ) -> runtara_environment::runner::Result<runtara_environment::runner::RunnerHandle> {
        use std::sync::atomic::Ordering;
        let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(120)).await;
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        Ok(runtara_environment::runner::RunnerHandle {
            handle_id: format!("probe-{}", options.instance_id),
            instance_id: options.instance_id.clone(),
            tenant_id: options.tenant_id.clone(),
            started_at: Utc::now(),
            metrics: None,
        })
    }

    async fn is_running(&self, _handle: &runtara_environment::runner::RunnerHandle) -> bool {
        true
    }

    async fn stop(
        &self,
        _handle: &runtara_environment::runner::RunnerHandle,
    ) -> runtara_environment::runner::Result<()> {
        Ok(())
    }

    async fn collect_result(
        &self,
        _handle: &runtara_environment::runner::RunnerHandle,
    ) -> (
        Option<serde_json::Value>,
        Option<String>,
        runtara_environment::runner::ContainerMetrics,
    ) {
        (
            None,
            None,
            runtara_environment::runner::ContainerMetrics::default(),
        )
    }
}

/// A batch must be relaunched concurrently, and never beyond the configured
/// bound.
///
/// Waking one instance at a time is what held the scheduler to
/// `batch_size / poll_interval` regardless of how idle the host was; spreading
/// the batch is what lets a drain run at the speed of the box. The upper bound
/// matters just as much — an unbounded fan-out would dump a whole backlog into
/// the runner at once after an outage.
#[tokio::test]
async fn a_batch_is_woken_concurrently_and_stays_within_its_bound() {
    skip_if_no_db!();
    let pool = get_test_pool().await;
    let tenant_id = "wake-concurrency-tenant";
    let image_id = create_test_image(&pool, tenant_id).await;

    let mut ids = Vec::new();
    for _ in 0..24 {
        ids.push(park_due_instance(&pool, tenant_id, &image_id).await);
    }

    let in_flight = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let peak = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    const BOUND: usize = 6;

    let persistence = Arc::new(PostgresPersistence::new(pool.clone()));
    let scheduler = WakeScheduler::new(
        pool.clone(),
        persistence,
        Arc::new(ConcurrencyProbeRunner {
            in_flight: Arc::clone(&in_flight),
            peak: Arc::clone(&peak),
        }),
        WakeSchedulerConfig {
            poll_interval: Duration::from_millis(50),
            batch_size: 24,
            concurrency: BOUND,
            claim_lease: Duration::from_secs(300),
            core_addr: "127.0.0.1:8001".to_string(),
            data_dir: PathBuf::from(".data"),
        },
    );
    let shutdown = scheduler.shutdown_handle();
    let handle = tokio::spawn(scheduler.run());

    tokio::time::sleep(Duration::from_secs(3)).await;
    shutdown.notify_one();
    let _ = handle.await;

    let observed = peak.load(std::sync::atomic::Ordering::SeqCst);
    assert!(
        observed > 1,
        "the batch must be spread across tasks, not awaited one at a time \
         (peak in-flight was {observed})"
    );
    assert!(
        observed <= BOUND,
        "in-batch concurrency must respect its bound: peak {observed} > {BOUND}"
    );

    for id in &ids {
        cleanup(&pool, id).await;
    }
    cleanup_image(&pool, &image_id).await;
}
