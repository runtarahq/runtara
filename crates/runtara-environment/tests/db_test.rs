// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Database operations tests for runtara-environment.
//!
//! These tests verify the correctness of database CRUD operations.

mod common;

use runtara_core::persistence::{CompleteInstanceParams, Persistence, PostgresPersistence};
use runtara_environment::db;
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

async fn get_pool() -> Option<sqlx::PgPool> {
    let database_url = std::env::var("TEST_ENVIRONMENT_DATABASE_URL")
        .or_else(|_| std::env::var("RUNTARA_ENVIRONMENT_DATABASE_URL"))
        .expect("db-integration-tests requires an environment database URL");
    let pool = sqlx::PgPool::connect(&database_url)
        .await
        .expect("required environment test database must accept connections");
    runtara_environment::migrations::run(&pool)
        .await
        .expect("required combined core/environment migrations must succeed");
    Some(pool)
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
    db::associate_instance_image(pool, instance_id, image_id, tenant_id, env, None)
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
        if let Some(checkpoint_id) = checkpoint_id {
            params = params.with_checkpoint(checkpoint_id);
        }
        persistence
            .complete_instance(params)
            .await
            .expect("Failed to complete instance");
        return;
    }
    let started_at = (status == "running").then(chrono::Utc::now);
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

/// Create a test image with a unique name
async fn create_test_image(
    pool: &sqlx::PgPool,
    image_id: &str,
    tenant_id: &str,
) -> Result<(), sqlx::Error> {
    let image_name = format!("test-image-{}", image_id);
    sqlx::query(
        "INSERT INTO images (image_id, tenant_id, name, binary_path) VALUES ($1, $2, $3, '/test')",
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
        statuses: Some(vec!["completed".to_string()]),
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

#[tokio::test]
async fn test_list_instances_by_multiple_statuses() {
    skip_if_no_db!();
    let pool = get_pool().await.expect("Failed to connect to database");

    // Unique per run: the assertions count rows for this tenant, so a previous
    // run that died before its cleanup must not be able to skew them.
    let tenant_id = format!("test-tenant-list-multi-status-{}", Uuid::new_v4());
    let tenant_id = tenant_id.as_str();
    let image_id = Uuid::new_v4().to_string();

    create_test_image(&pool, &image_id, tenant_id)
        .await
        .expect("Failed to create test image");

    // One instance per status, so a filter that only honours its first entry
    // comes back short.
    let ids: Vec<_> = (0..3).map(|_| Uuid::new_v4().to_string()).collect();
    for id in &ids {
        create_test_instance(&pool, id, tenant_id, &image_id).await;
    }
    update_test_instance_status(&pool, &ids[0], "failed", None).await;
    update_test_instance_status(&pool, &ids[1], "cancelled", None).await;
    update_test_instance_status(&pool, &ids[2], "completed", None).await;

    let options = db::ListInstancesOptions {
        tenant_id: Some(tenant_id.to_string()),
        statuses: Some(vec!["failed".to_string(), "cancelled".to_string()]),
        limit: 100,
        ..Default::default()
    };

    let matched = db::list_instances(&pool, &options)
        .await
        .expect("Failed to list instances");
    let mut matched_ids: Vec<_> = matched.iter().map(|i| i.instance_id.clone()).collect();
    matched_ids.sort();
    let mut expected_ids = vec![ids[0].clone(), ids[1].clone()];
    expected_ids.sort();

    assert_eq!(matched_ids, expected_ids, "both statuses must be applied");

    // The count drives totalElements, so it has to agree with the page.
    let count = db::count_instances(&pool, &options)
        .await
        .expect("Failed to count instances");
    assert_eq!(count, 2);

    // An empty list means "no status filter", not "match nothing".
    let unfiltered = db::ListInstancesOptions {
        tenant_id: Some(tenant_id.to_string()),
        statuses: Some(Vec::new()),
        limit: 100,
        ..Default::default()
    };
    assert_eq!(
        db::count_instances(&pool, &unfiltered)
            .await
            .expect("Failed to count instances"),
        3
    );

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

#[tokio::test]
async fn test_instance_timeout_seconds_round_trips() {
    skip_if_no_db!();
    let pool = get_pool().await.expect("Failed to connect to database");

    let instance_id = Uuid::new_v4().to_string();
    let tenant_id = "test-tenant-timeout";
    let image_id = Uuid::new_v4().to_string();

    create_test_image(&pool, &image_id, tenant_id)
        .await
        .expect("Failed to create test image");

    let persistence = PostgresPersistence::new(pool.clone());
    persistence
        .register_instance(&instance_id, tenant_id)
        .await
        .expect("Failed to register instance");

    // Persist a per-instance timeout larger than the legacy hardcoded 300s.
    db::associate_instance_image(&pool, &instance_id, &image_id, tenant_id, None, Some(1800))
        .await
        .expect("Failed to associate instance image");

    let timeout = db::get_instance_timeout_seconds(&pool, &instance_id)
        .await
        .expect("Query should succeed");
    assert_eq!(timeout, Some(1800), "Persisted timeout should round-trip");

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
async fn test_instance_timeout_seconds_absent_is_none() {
    skip_if_no_db!();
    let pool = get_pool().await.expect("Failed to connect to database");

    let instance_id = Uuid::new_v4().to_string();
    let tenant_id = "test-tenant-timeout-none";
    let image_id = Uuid::new_v4().to_string();

    create_test_image(&pool, &image_id, tenant_id)
        .await
        .expect("Failed to create test image");

    // Associate without a timeout (e.g. rows predating the column).
    create_test_instance(&pool, &instance_id, tenant_id, &image_id).await;

    let timeout = db::get_instance_timeout_seconds(&pool, &instance_id)
        .await
        .expect("Query should succeed");
    assert_eq!(timeout, None, "Absent timeout should read back as None");

    // A nonexistent instance is also None (no row).
    let missing = db::get_instance_timeout_seconds(&pool, "nonexistent-instance")
        .await
        .expect("Query should succeed");
    assert_eq!(missing, None);

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

// ============================================================================
// Tenant metrics aggregation
//
// The aggregation buckets by flooring the Unix epoch to a multiple of the
// requested width. These tests pin the two properties that flooring has to
// hold on to - the empty-bucket spine and the join alignment between spine and
// aggregate - because when either breaks the query still returns rows, just
// wrong ones.
// ============================================================================

/// Seed a terminal instance with timestamps the test chooses.
///
/// `complete_instance` stamps `finished_at` with `NOW()`, which is exactly what
/// production wants and exactly what a bucketing test cannot use. Writing the
/// three columns directly is the honest way to get a controlled fixture.
async fn seed_terminal_instance(
    pool: &PgPool,
    tenant_id: &str,
    status: &str,
    started_at: chrono::DateTime<chrono::Utc>,
    finished_at: chrono::DateTime<chrono::Utc>,
    memory_peak_bytes: Option<i64>,
) -> String {
    let instance_id = format!("metrics-{}", Uuid::new_v4());
    PostgresPersistence::new(pool.clone())
        .register_instance(&instance_id, tenant_id)
        .await
        .expect("Failed to register instance");

    sqlx::query(
        "UPDATE instances
            SET status = $2::instance_status,
                started_at = $3,
                finished_at = $4,
                memory_peak_bytes = $5
          WHERE instance_id = $1",
    )
    .bind(&instance_id)
    .bind(status)
    .bind(started_at)
    .bind(finished_at)
    .bind(memory_peak_bytes)
    .execute(pool)
    .await
    .expect("Failed to stamp instance timestamps");

    instance_id
}

async fn delete_tenant_instances(pool: &PgPool, tenant_id: &str) {
    sqlx::query("DELETE FROM instances WHERE tenant_id = $1")
        .bind(tenant_id)
        .execute(pool)
        .await
        .ok();
}

/// A fixed, aligned instant so bucket boundaries are arithmetic, not clock luck.
fn epoch(seconds: i64) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::from_timestamp(seconds, 0).expect("timestamp in range")
}

#[tokio::test]
async fn test_tenant_metrics_one_minute_buckets_over_an_hour() {
    skip_if_no_db!();
    let Some(pool) = get_pool().await else { return };
    let tenant_id = format!("tenant-{}", Uuid::new_v4());

    // Window: epoch 0 .. 3600. Runs land in the buckets starting at 0s, 120s
    // and 3540s - the last one is the final whole minute of the window.
    let start = epoch(0);
    let end = epoch(3_600);
    seed_terminal_instance(
        &pool,
        &tenant_id,
        "completed",
        epoch(0),
        epoch(10),
        Some(1_048_576),
    )
    .await;
    seed_terminal_instance(
        &pool,
        &tenant_id,
        "completed",
        epoch(100),
        epoch(130),
        Some(2_097_152),
    )
    .await;
    seed_terminal_instance(&pool, &tenant_id, "failed", epoch(120), epoch(150), None).await;
    seed_terminal_instance(
        &pool,
        &tenant_id,
        "cancelled",
        epoch(3_500),
        epoch(3_580),
        None,
    )
    .await;

    let buckets = db::get_tenant_metrics(
        &pool,
        &db::TenantMetricsOptions {
            tenant_id: tenant_id.clone(),
            start_time: start,
            end_time: end,
            bucket_seconds: 60,
        },
    )
    .await
    .expect("aggregation should succeed");

    // 60 whole minutes, and the spine is inclusive of both edges.
    assert_eq!(buckets.len(), 61, "expected a full minute-resolution spine");

    // Every bucket start is a whole minute, and they ascend without gaps.
    for (index, bucket) in buckets.iter().enumerate() {
        assert_eq!(
            bucket.bucket_time.timestamp(),
            index as i64 * 60,
            "bucket {index} is misaligned"
        );
    }

    let at_minute = |minute: usize| &buckets[minute];
    assert_eq!(at_minute(0).invocation_count, 1);
    assert_eq!(at_minute(0).success_count, 1);
    assert_eq!(at_minute(2).invocation_count, 2, "60s..180s holds two runs");
    assert_eq!(at_minute(2).success_count, 1);
    assert_eq!(at_minute(2).failure_count, 1);
    assert_eq!(at_minute(59).cancelled_count, 1);

    // Empty buckets are present and zeroed rather than absent.
    assert_eq!(at_minute(30).invocation_count, 0);
    assert_eq!(at_minute(30).success_count, 0);

    delete_tenant_instances(&pool, &tenant_id).await;
}

#[tokio::test]
async fn test_tenant_metrics_empty_buckets_carry_null_aggregates_not_zero() {
    skip_if_no_db!();
    let Some(pool) = get_pool().await else { return };
    let tenant_id = format!("tenant-{}", Uuid::new_v4());

    seed_terminal_instance(
        &pool,
        &tenant_id,
        "completed",
        epoch(0),
        epoch(30),
        Some(4_194_304),
    )
    .await;

    let buckets = db::get_tenant_metrics(
        &pool,
        &db::TenantMetricsOptions {
            tenant_id: tenant_id.clone(),
            start_time: epoch(0),
            end_time: epoch(600),
            bucket_seconds: 60,
        },
    )
    .await
    .expect("aggregation should succeed");

    // "No runs" and "runs that took no time" are different claims. A zero here
    // would be averaged into the dashboard's duration and memory figures.
    let populated = &buckets[0];
    assert_eq!(populated.invocation_count, 1);
    assert_eq!(populated.avg_duration_ms, Some(30_000.0));
    assert_eq!(populated.avg_memory_bytes, Some(4_194_304.0));
    assert_eq!(populated.max_memory_bytes, Some(4_194_304));

    let empty = &buckets[5];
    assert_eq!(empty.invocation_count, 0);
    assert!(
        empty.avg_duration_ms.is_none(),
        "empty bucket claimed a duration"
    );
    assert!(
        empty.avg_memory_bytes.is_none(),
        "empty bucket claimed memory"
    );
    assert!(empty.max_memory_bytes.is_none());

    delete_tenant_instances(&pool, &tenant_id).await;
}

#[tokio::test]
async fn test_tenant_metrics_hourly_width_aligns_to_hour_boundaries() {
    skip_if_no_db!();
    let Some(pool) = get_pool().await else { return };
    let tenant_id = format!("tenant-{}", Uuid::new_v4());

    seed_terminal_instance(&pool, &tenant_id, "completed", epoch(100), epoch(200), None).await;
    seed_terminal_instance(
        &pool,
        &tenant_id,
        "completed",
        epoch(7_300),
        epoch(7_400),
        None,
    )
    .await;

    let buckets = db::get_tenant_metrics(
        &pool,
        &db::TenantMetricsOptions {
            tenant_id: tenant_id.clone(),
            start_time: epoch(0),
            end_time: epoch(10_800),
            bucket_seconds: 3_600,
        },
    )
    .await
    .expect("aggregation should succeed");

    // Flooring the epoch by 3600 must reproduce what date_trunc('hour') gave,
    // since hours divide the epoch evenly. This is the compatibility pin.
    assert_eq!(buckets.len(), 4);
    for bucket in &buckets {
        assert_eq!(
            bucket.bucket_time.timestamp() % 3_600,
            0,
            "hourly bucket not on an hour boundary"
        );
    }
    assert_eq!(buckets[0].invocation_count, 1);
    assert_eq!(buckets[1].invocation_count, 0);
    assert_eq!(buckets[2].invocation_count, 1);

    delete_tenant_instances(&pool, &tenant_id).await;
}

#[tokio::test]
async fn test_tenant_metrics_daily_buckets_stay_utc_under_a_shifted_session_timezone() {
    skip_if_no_db!();
    let Some(pool) = get_pool().await else { return };
    let tenant_id = format!("tenant-{}", Uuid::new_v4());

    // Two claims, and the first is what justifies the change.
    //
    // `date_trunc('day', timestamptz)` truncates in the *session* time zone,
    // and nothing in this codebase pins one - so the aggregation's daily
    // boundaries used to move with whatever the server happened to be set to,
    // while `bucket_time` was reported to callers as UTC regardless. A
    // half-hour-offset zone makes that impossible to mistake for a whole-hour
    // coincidence.
    //
    // Asserted on one explicitly-held connection because a `SET TIME ZONE`
    // applies to a session, and a pooled call is free to run on a different
    // one. Testing the expressions here, rather than hoping the pool hands
    // back the connection we configured, is what keeps this test honest.
    let day = 86_400i64;
    let mut conn = pool.acquire().await.expect("connection");
    sqlx::query("SET TIME ZONE 'Asia/Kolkata'")
        .execute(&mut *conn)
        .await
        .expect("session timezone should be settable");

    let (truncated, floored): (f64, f64) = sqlx::query_as(
        "SELECT
             extract(epoch FROM date_trunc('day', to_timestamp($1::float8)))::float8,
             floor(extract(epoch FROM to_timestamp($1::float8))::float8 / 86400) * 86400",
    )
    .bind(day as f64)
    .fetch_one(&mut *conn)
    .await
    .expect("expression comparison should succeed");

    assert_ne!(
        truncated, floored,
        "date_trunc no longer drifts under a shifted session zone - if Postgres \
         changed this, the rationale for flooring the epoch needs revisiting"
    );
    assert_eq!(
        truncated, 66_600.0,
        "expected date_trunc to land 5h30m early under Asia/Kolkata"
    );
    assert_eq!(
        floored, day as f64,
        "flooring the epoch must be UTC-absolute"
    );
    drop(conn);

    // And the aggregation itself keeps UTC-aligned days.
    seed_terminal_instance(
        &pool,
        &tenant_id,
        "completed",
        epoch(day),
        epoch(day + 60),
        None,
    )
    .await;

    let buckets = db::get_tenant_metrics(
        &pool,
        &db::TenantMetricsOptions {
            tenant_id: tenant_id.clone(),
            start_time: epoch(0),
            end_time: epoch(3 * day),
            bucket_seconds: 86_400,
        },
    )
    .await
    .expect("aggregation should succeed");

    for bucket in &buckets {
        assert_eq!(
            bucket.bucket_time.timestamp() % day,
            0,
            "daily bucket drifted off UTC midnight: {}",
            bucket.bucket_time
        );
    }
    assert_eq!(
        buckets[1].invocation_count, 1,
        "the run should sit in the UTC day that contains it"
    );

    delete_tenant_instances(&pool, &tenant_id).await;
}

#[tokio::test]
async fn test_tenant_metrics_counts_a_boundary_crossing_run_once() {
    skip_if_no_db!();
    let Some(pool) = get_pool().await else { return };
    let tenant_id = format!("tenant-{}", Uuid::new_v4());

    // Starts in the 0s bucket, finishes in the 120s one. Aggregation keys on
    // finished_at, so it belongs to the later bucket and to only that bucket.
    seed_terminal_instance(&pool, &tenant_id, "completed", epoch(30), epoch(150), None).await;

    let buckets = db::get_tenant_metrics(
        &pool,
        &db::TenantMetricsOptions {
            tenant_id: tenant_id.clone(),
            start_time: epoch(0),
            end_time: epoch(600),
            bucket_seconds: 60,
        },
    )
    .await
    .expect("aggregation should succeed");

    let total: i64 = buckets.iter().map(|b| b.invocation_count).sum();
    assert_eq!(total, 1, "a run spanning a boundary was counted twice");
    assert_eq!(
        buckets[2].invocation_count, 1,
        "not in the finished_at bucket"
    );
    assert_eq!(buckets[0].invocation_count, 0);
    // Duration still spans the whole run, not the part inside the bucket.
    assert_eq!(buckets[2].avg_duration_ms, Some(120_000.0));

    delete_tenant_instances(&pool, &tenant_id).await;
}

#[tokio::test]
async fn test_tenant_metrics_totals_do_not_change_with_bucket_width() {
    skip_if_no_db!();
    let Some(pool) = get_pool().await else { return };
    let tenant_id = format!("tenant-{}", Uuid::new_v4());

    // Scatter runs across a day, deliberately off any round boundary.
    let mut expected_total = 0i64;
    for offset in [7i64, 61, 199, 3_607, 7_411, 43_205, 80_000, 86_399] {
        seed_terminal_instance(
            &pool,
            &tenant_id,
            if offset % 3 == 0 {
                "failed"
            } else {
                "completed"
            },
            epoch(offset.saturating_sub(5)),
            epoch(offset),
            Some(1_048_576),
        )
        .await;
        expected_total += 1;
    }

    // The property that catches a spine/aggregate misalignment: the same window
    // must total the same at every width. If the two sides of the LEFT JOIN
    // ever key differently, runs silently vanish into unmatched buckets.
    for width in [60u32, 360, 1_440, 3_600, 7_200, 21_600, 86_400] {
        let buckets = db::get_tenant_metrics(
            &pool,
            &db::TenantMetricsOptions {
                tenant_id: tenant_id.clone(),
                start_time: epoch(0),
                end_time: epoch(86_400),
                bucket_seconds: width,
            },
        )
        .await
        .expect("aggregation should succeed");

        let total: i64 = buckets.iter().map(|b| b.invocation_count).sum();
        assert_eq!(
            total, expected_total,
            "width {width}s lost or duplicated runs"
        );

        let terminal: i64 = buckets
            .iter()
            .map(|b| b.success_count + b.failure_count + b.cancelled_count)
            .sum();
        assert_eq!(
            terminal, expected_total,
            "width {width}s split the statuses"
        );
    }

    delete_tenant_instances(&pool, &tenant_id).await;
}

#[tokio::test]
async fn test_tenant_metrics_excludes_other_tenants_and_non_terminal_runs() {
    skip_if_no_db!();
    let Some(pool) = get_pool().await else { return };
    let tenant_id = format!("tenant-{}", Uuid::new_v4());
    let other_tenant = format!("tenant-{}", Uuid::new_v4());

    seed_terminal_instance(&pool, &tenant_id, "completed", epoch(0), epoch(30), None).await;
    seed_terminal_instance(&pool, &other_tenant, "completed", epoch(0), epoch(30), None).await;
    // Running: no finished_at, so it is invisible to the aggregation by design.
    seed_terminal_instance(&pool, &tenant_id, "running", epoch(0), epoch(30), None).await;

    let buckets = db::get_tenant_metrics(
        &pool,
        &db::TenantMetricsOptions {
            tenant_id: tenant_id.clone(),
            start_time: epoch(0),
            end_time: epoch(600),
            bucket_seconds: 60,
        },
    )
    .await
    .expect("aggregation should succeed");

    let total: i64 = buckets.iter().map(|b| b.invocation_count).sum();
    assert_eq!(
        total, 1,
        "aggregation crossed a tenant or counted a live run"
    );

    delete_tenant_instances(&pool, &tenant_id).await;
    delete_tenant_instances(&pool, &other_tenant).await;
}
