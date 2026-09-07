// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Postgres-backed tests for the tenant-metrics aggregation.
//!
//! These live beside the query instead of in `tests/` because they exercise it
//! below the handler that guards it. `handle_get_tenant_metrics` refuses more
//! than [`crate::handlers::MAX_METRIC_BUCKETS`] buckets, and the finest-width
//! case here deliberately asks for 1441 — the width most likely to expose a
//! misalignment between the empty-bucket spine and the aggregate. They also
//! assert on `MetricsBucketRow`'s `Option<f64>` memory figures, which the
//! handler narrows to `Option<i64>`. Routing them through the public API would
//! mean rewriting the assertions to fit the refactor.

use runtara_core::persistence::Persistence;
use runtara_store_postgres::PostgresPersistence;
use uuid::Uuid;

use super::*;

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
    let pool = crate::test_support::pool().await;
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

    let buckets = get_tenant_metrics(&pool, &tenant_id, start, end, 60)
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
    let pool = crate::test_support::pool().await;
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

    let buckets = get_tenant_metrics(&pool, &tenant_id, epoch(0), epoch(600), 60)
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
    let pool = crate::test_support::pool().await;
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

    let buckets = get_tenant_metrics(&pool, &tenant_id, epoch(0), epoch(10_800), 3_600)
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
    let pool = crate::test_support::pool().await;
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

    let buckets = get_tenant_metrics(&pool, &tenant_id, epoch(0), epoch(3 * day), 86_400)
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
    let pool = crate::test_support::pool().await;
    let tenant_id = format!("tenant-{}", Uuid::new_v4());

    // Starts in the 0s bucket, finishes in the 120s one. Aggregation keys on
    // finished_at, so it belongs to the later bucket and to only that bucket.
    seed_terminal_instance(&pool, &tenant_id, "completed", epoch(30), epoch(150), None).await;

    let buckets = get_tenant_metrics(&pool, &tenant_id, epoch(0), epoch(600), 60)
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
    let pool = crate::test_support::pool().await;
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
        let buckets = get_tenant_metrics(&pool, &tenant_id, epoch(0), epoch(86_400), width)
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
    let pool = crate::test_support::pool().await;
    let tenant_id = format!("tenant-{}", Uuid::new_v4());
    let other_tenant = format!("tenant-{}", Uuid::new_v4());

    seed_terminal_instance(&pool, &tenant_id, "completed", epoch(0), epoch(30), None).await;
    seed_terminal_instance(&pool, &other_tenant, "completed", epoch(0), epoch(30), None).await;
    // Running: no finished_at, so it is invisible to the aggregation by design.
    seed_terminal_instance(&pool, &tenant_id, "running", epoch(0), epoch(30), None).await;

    let buckets = get_tenant_metrics(&pool, &tenant_id, epoch(0), epoch(600), 60)
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
