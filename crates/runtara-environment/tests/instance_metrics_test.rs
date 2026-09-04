// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Tests for the forensic columns the runner records about a process.
//!
//! Moved here from runtara-core with the writes themselves: peak memory, CPU
//! time and captured stderr are the runner's observations, and Core never reads
//! any of them back to decide anything.

use runtara_core::persistence::Persistence;
use runtara_environment::metrics;
use runtara_store_postgres::PostgresPersistence;
use sqlx::PgPool;
use uuid::Uuid;

macro_rules! skip_if_no_db {
    () => {
        assert!(
            std::env::var("TEST_ENVIRONMENT_DATABASE_URL").is_ok()
                || std::env::var("RUNTARA_ENVIRONMENT_DATABASE_URL").is_ok(),
            "db-integration-tests requires TEST_ENVIRONMENT_DATABASE_URL or RUNTARA_ENVIRONMENT_DATABASE_URL"
        );
    };
}

async fn test_pool() -> PgPool {
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

async fn registered_instance(pool: &PgPool, kind: &str) -> String {
    let id = format!("env-metrics-{kind}-{}", Uuid::new_v4());
    PostgresPersistence::new(pool.clone())
        .register_instance(&id, &format!("env-metrics-tenant-{kind}"))
        .await
        .unwrap();
    id
}

/// The second write is where the semantics live: the update is
/// `SET x = COALESCE(x, $n)`, so the first non-NULL observation sticks and
/// every later one is ignored rather than overwriting it.
#[tokio::test]
async fn resource_metrics_keep_the_first_observation() {
    skip_if_no_db!();
    let pool = test_pool().await;
    let instance_id = registered_instance(&pool, "resources").await;

    metrics::record_instance_resources(&pool, &instance_id, Some(1024 * 1024), Some(500_000))
        .await
        .expect("failed to record resources");

    let read = |id: String, pool: PgPool| async move {
        sqlx::query_as::<_, (Option<i64>, Option<i64>)>(
            "SELECT memory_peak_bytes, cpu_usage_usec FROM instances WHERE instance_id = $1",
        )
        .bind(&id)
        .fetch_one(&pool)
        .await
        .unwrap()
    };

    let row = read(instance_id.clone(), pool.clone()).await;
    assert_eq!(row.0, Some(1024 * 1024));
    assert_eq!(row.1, Some(500_000));

    metrics::record_instance_resources(&pool, &instance_id, Some(9_999_999), Some(1))
        .await
        .expect("failed to record resources");

    let row = read(instance_id.clone(), pool.clone()).await;
    assert_eq!(
        row.0,
        Some(1024 * 1024),
        "COALESCE keeps the first recorded memory peak"
    );
    assert_eq!(
        row.1,
        Some(500_000),
        "COALESCE keeps the first recorded CPU usage"
    );

    let _ = sqlx::query("DELETE FROM instances WHERE instance_id = $1")
        .bind(&instance_id)
        .execute(&pool)
        .await;
}

/// Nothing to write means no statement and no report.
#[tokio::test]
async fn recording_no_resources_is_a_no_op() {
    skip_if_no_db!();
    let pool = test_pool().await;
    let instance_id = registered_instance(&pool, "noop").await;

    metrics::record_instance_resources(&pool, &instance_id, None, None)
        .await
        .expect("recording nothing must succeed");

    let row: (Option<i64>, Option<i64>) = sqlx::query_as(
        "SELECT memory_peak_bytes, cpu_usage_usec FROM instances WHERE instance_id = $1",
    )
    .bind(&instance_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row, (None, None));

    let _ = sqlx::query("DELETE FROM instances WHERE instance_id = $1")
        .bind(&instance_id)
        .execute(&pool)
        .await;
}

/// The first capture is what explained the failure, so a later one is ignored.
#[tokio::test]
async fn stderr_keeps_the_first_capture() {
    skip_if_no_db!();
    let pool = test_pool().await;
    let instance_id = registered_instance(&pool, "stderr").await;

    metrics::record_instance_stderr(&pool, &instance_id, "Error: something went wrong\n")
        .await
        .expect("failed to record stderr");
    metrics::record_instance_stderr(&pool, &instance_id, "second capture\n")
        .await
        .expect("failed to record stderr");

    let row: (Option<String>,) =
        sqlx::query_as("SELECT stderr FROM instances WHERE instance_id = $1")
            .bind(&instance_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        row.0,
        Some("Error: something went wrong\n".to_string()),
        "COALESCE keeps the first captured stderr"
    );

    let _ = sqlx::query("DELETE FROM instances WHERE instance_id = $1")
        .bind(&instance_id)
        .execute(&pool)
        .await;
}

/// The combined write returns the status the guest reported, which is how the
/// container monitor tells a normal exit from a crash.
#[tokio::test]
async fn returning_status_reports_the_guest_status() {
    skip_if_no_db!();
    let pool = test_pool().await;
    let instance_id = registered_instance(&pool, "returning").await;

    let observed =
        metrics::record_resources_returning_status(&pool, &instance_id, Some(2048), Some(7))
            .await
            .expect("failed to record resources");
    let (status, _reason) = observed.expect("a registered instance must return a status");
    assert_eq!(status, "pending");

    assert!(
        metrics::record_resources_returning_status(&pool, "no-such-instance", None, None)
            .await
            .expect("a missing row is not an error")
            .is_none(),
        "an unknown instance must report no status"
    );

    let _ = sqlx::query("DELETE FROM instances WHERE instance_id = $1")
        .bind(&instance_id)
        .execute(&pool)
        .await;
}
