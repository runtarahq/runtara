use super::*;
use runtara_core::domain::InstanceStatus;
use runtara_core::persistence::{CompleteInstanceParams, Persistence};
use runtara_store_postgres::PostgresPersistence;
use sqlx::postgres::PgPoolOptions;

// Queue drains are global by design. Each test owns an isolated database so
// concurrent workers cannot consume another test's facts. Retain it for inspection.
async fn pool() -> PgPool {
    migrated_pool(true).await
}

async fn migrated_pool(include_usage: bool) -> PgPool {
    let admin = crate::test_support::pool().await;
    let database = format!("usage_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE {database}"))
        .execute(&admin)
        .await
        .unwrap();
    let options = (*admin.connect_options()).clone().database(&database);
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await
        .unwrap();
    let mut migrator = crate::migrations::migrator().await.unwrap();
    if !include_usage {
        migrator.migrations = std::borrow::Cow::Owned(
            migrator
                .iter()
                .filter(|migration| migration.version != 29)
                .cloned()
                .collect(),
        );
    }
    migrator.run(&pool).await.unwrap();
    pool
}

async fn drain(
    pool: &PgPool,
    cutoff: DateTime<Utc>,
    batch: i64,
    return_facts: bool,
) -> Result<Vec<UsageFact>, sqlx::Error> {
    super::drain(pool, cutoff, batch, return_facts)
        .await
        .map(|batch| batch.facts)
}

fn at(seconds: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(seconds, 0).unwrap()
}

async fn register(pool: &PgPool, id: &str) {
    PostgresPersistence::new(pool.clone())
        .register_instance(id, "usage-tenant")
        .await
        .unwrap();
}

async fn finish(pool: &PgPool, id: &str, end: i64) {
    sqlx::query("UPDATE instances SET status = 'completed', started_at = $2 - interval '2 seconds', finished_at = $2 WHERE instance_id = $1")
        .bind(id).bind(at(end)).execute(pool).await.unwrap();
}

async fn count(pool: &PgPool, table: &str) -> i64 {
    sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn completion_and_late_resources_are_once_only_and_survive_cleanup() {
    let pool = pool().await;
    register(&pool, "first").await;
    finish(&pool, "first", 130).await;
    // A repeated writer changes raw finished_at. Usage keeps the first bucket.
    finish(&pool, "first", 310).await;
    let facts = drain(&pool, at(0), 1, true).await.unwrap();
    assert_eq!(facts.len(), 1);
    assert!(facts[0].completion && facts[0].export);
    assert_eq!(facts[0].duration_ms, Some(2000.0));
    assert!(drain(&pool, at(0), 1, true).await.unwrap().is_empty());

    let resources = crate::instance_repository::InstanceRepository::new(pool.clone());
    resources
        .record_resources_returning_status("first", Some(4096), None)
        .await
        .unwrap();
    resources
        .record_resources_returning_status("first", Some(8192), Some(500_000))
        .await
        .unwrap();
    resources
        .record_resources_returning_status("first", Some(8192), Some(900_000))
        .await
        .unwrap();
    assert_eq!(count(&pool, "usage_pending").await, 2);
    sqlx::query("DELETE FROM instances WHERE instance_id = 'first'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(count(&pool, "instances").await, 0);
    let facts = drain(&pool, at(0), 10, true).await.unwrap();
    assert_eq!(facts.len(), 2);
    assert!(
        facts
            .iter()
            .all(|f| !f.completion && f.export && f.duration_ms.is_none())
    );
    assert_eq!(
        facts.iter().filter_map(|f| f.memory_bytes).sum::<i64>(),
        4096
    );
    assert_eq!(
        facts.iter().filter_map(|f| f.cpu_usec).sum::<i64>(),
        500_000
    );
    let buckets = crate::db::get_tenant_metrics(&pool, "usage-tenant", at(0), at(600), 60)
        .await
        .unwrap();
    let b = &buckets[2];
    assert_eq!(b.invocation_count, 1);
    assert_eq!(b.duration_observation_count, 1);
    assert_eq!(b.memory_observation_count, 1);
    assert_eq!(b.cpu_observation_count, 1);
    assert_eq!(b.avg_memory_bytes, Some(4096.0));
    assert_eq!(b.avg_cpu_seconds, Some(0.5));
    assert_eq!(buckets[5].invocation_count, 0);
}

#[tokio::test]
async fn lifecycle_rollback_and_aggregation_failure_leave_no_partial_contribution() {
    let pool = pool().await;
    register(&pool, "rollback").await;
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("UPDATE instances SET status = 'failed', finished_at = now() WHERE instance_id = 'rollback'")
        .execute(&mut *tx).await.unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(count(&pool, "usage_pending").await, 0);
    finish(&pool, "rollback", 130).await;
    sqlx::query(
        "ALTER TABLE usage_minutes ADD CONSTRAINT simulate_failure CHECK (invocation_count < 0)",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert!(drain(&pool, at(0), 10, true).await.is_err());
    assert_eq!(count(&pool, "usage_pending").await, 1);
    assert_eq!(count(&pool, "usage_minutes").await, 0);
    sqlx::query("ALTER TABLE usage_minutes DROP CONSTRAINT simulate_failure")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(drain(&pool, at(0), 10, true).await.unwrap().len(), 1);
    assert!(drain(&pool, at(0), 10, true).await.unwrap().is_empty());
}

#[tokio::test]
async fn backfill_and_delete_capture_pre_upgrade_rows_without_replaying_otel() {
    let pool = migrated_pool(false).await;
    for id in ["backfill-a", "backfill-b", "deleted-before-backfill"] {
        register(&pool, id).await;
        finish(&pool, id, 130).await;
    }
    // Applies the forward migration to populated pre-upgrade tables, including
    // databases that already have newer timestamped environment migrations.
    crate::migrations::run(&pool).await.unwrap();
    backfill(&pool, 1).await.unwrap();
    assert_eq!(
        count(&pool, "usage_pending").await,
        1,
        "backfill is bounded"
    );
    sqlx::query("DELETE FROM instances WHERE instance_id = 'deleted-before-backfill'")
        .execute(&pool)
        .await
        .unwrap();
    backfill(&pool, 1).await.unwrap();
    backfill(&pool, 1).await.unwrap();
    let facts = drain(&pool, at(0), 10, true).await.unwrap();
    assert_eq!(facts.len(), 3);
    assert!(facts.iter().all(|f| f.completion && !f.export));
    sqlx::query("DELETE FROM instances")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(count(&pool, "usage_pending").await, 0);
    let total: i64 = sqlx::query_scalar("SELECT invocation_count FROM usage_minutes")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(total, 3);
}

#[tokio::test]
async fn concurrent_bounded_drains_and_retention_do_not_duplicate_or_resurrect_history() {
    let pool = pool().await;
    for id in ["a", "b", "c"] {
        register(&pool, id).await;
        finish(&pool, id, 130).await;
    }
    let (a, b) = tokio::join!(drain(&pool, at(0), 1, true), drain(&pool, at(0), 1, true));
    assert_eq!(a.unwrap().len() + b.unwrap().len(), 2);
    assert_eq!(count(&pool, "usage_pending").await, 1);
    drain(&pool, at(0), 1, true).await.unwrap();
    let total: i64 = sqlx::query_scalar("SELECT invocation_count FROM usage_minutes")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(total, 3);
    expire(&pool, at(180), 1).await.unwrap();
    assert_eq!(count(&pool, "usage_minutes").await, 0);
    crate::instance_repository::InstanceRepository::new(pool.clone())
        .record_resources_returning_status("a", Some(4096), None)
        .await
        .unwrap();
    drain(&pool, at(180), 1, true).await.unwrap();
    assert_eq!(
        count(&pool, "usage_minutes").await,
        0,
        "late resources cannot resurrect expired buckets"
    );
}

#[tokio::test]
async fn guarded_lifecycle_writers_capture_all_outcomes_and_ignore_suspensions() {
    let pool = pool().await;
    let persistence = PostgresPersistence::new(pool.clone());
    for (id, status) in [
        ("ok", InstanceStatus::Completed),
        ("bad", InstanceStatus::Failed),
        ("cancel", InstanceStatus::Cancelled),
    ] {
        register(&pool, id).await;
        // The skipped guarded write must not emit any fact.
        assert!(
            !persistence
                .complete_instance(CompleteInstanceParams::new(id, status).if_running())
                .await
                .unwrap()
        );
        persistence
            .update_instance_status(id, InstanceStatus::Running, Some(Utc::now()))
            .await
            .unwrap();
        assert!(
            persistence
                .complete_instance(CompleteInstanceParams::new(id, status).if_running())
                .await
                .unwrap()
        );
        assert!(
            !persistence
                .complete_instance(CompleteInstanceParams::new(id, status).if_running())
                .await
                .unwrap()
        );
    }
    register(&pool, "sleeping").await;
    persistence
        .complete_instance(CompleteInstanceParams::new(
            "sleeping",
            InstanceStatus::Suspended,
        ))
        .await
        .unwrap();
    let facts = drain(&pool, at(0), 10, true).await.unwrap();
    assert_eq!(facts.len(), 3);
    assert!(facts.iter().all(|f| f.completion && f.export));
    let totals: (i64, i64, i64, i64) = sqlx::query_as("SELECT sum(invocation_count)::bigint, sum(success_count)::bigint, sum(failure_count)::bigint, sum(cancelled_count)::bigint FROM usage_minutes").fetch_one(&pool).await.unwrap();
    assert_eq!(totals, (3, 1, 1, 1));
}

#[tokio::test]
async fn disabled_telemetry_returns_no_facts_but_retains_product_history() {
    let pool = pool().await;
    register(&pool, "disabled").await;
    finish(&pool, "disabled", 130).await;
    let result = super::drain(&pool, at(0), 10, false).await.unwrap();
    assert_eq!(result.count, 1);
    assert!(result.facts.is_empty());
    assert_eq!(count(&pool, "usage_minutes").await, 1);
    assert_eq!(count(&pool, "usage_pending").await, 0);
}
