// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Row-level security on the runtime database, core's tables and
//! Environment's alike: a table that is policed by tenant shows a connection
//! its tenant's rows, or every tenant's when it asks as the platform, and
//! refuses one that names neither.
//!
//! Requires the explicit `db-integration-tests` feature and a live Postgres.

use runtara_store_postgres::tenant_scope::{scoped, scoping, system};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

fn url() -> String {
    std::env::var("TEST_ENVIRONMENT_DATABASE_URL")
        .expect("db-integration-tests requires TEST_ENVIRONMENT_DATABASE_URL")
}

/// A database of its own, migrated from nothing: the order a new deployment
/// runs the migrations in, where Environment's tables are policed after
/// core's policies were rewritten.
async fn fresh() -> (PgPool, String) {
    let admin = PgPool::connect(&url()).await.unwrap();
    let name = format!("rls_runtime_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE {name}"))
        .execute(&admin)
        .await
        .unwrap();
    let options: PgConnectOptions = url().parse().unwrap();
    let pool = PgPool::connect_with(options.database(&name)).await.unwrap();
    runtara_environment::migrations::run(&pool)
        .await
        .expect("migrations");
    (pool, name)
}

async fn drop_database(pool: PgPool, name: &str) {
    pool.close().await;
    let admin = PgPool::connect(&url()).await.unwrap();
    sqlx::query(&format!("DROP DATABASE {name}"))
        .execute(&admin)
        .await
        .unwrap();
}

#[tokio::test]
async fn every_policy_asks_for_the_tenant_or_for_every_tenant_by_name() {
    let (pool, name) = fresh().await;

    let policed: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = 'public' AND c.relkind IN ('r', 'p') AND c.relforcerowsecurity",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        policed >= 15,
        "core's and Environment's tables are policed: {policed}"
    );

    // Each policed table checks reads and writes alike: its tenant's rows,
    // or every tenant's when the session may ask for that.
    let loose: Vec<String> = sqlx::query_scalar(
        "SELECT c.relname::text FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = 'public' AND c.relkind IN ('r', 'p') AND c.relforcerowsecurity \
           AND NOT EXISTS (SELECT 1 FROM pg_policies p \
                WHERE p.schemaname = 'public' AND p.tablename = c.relname \
                  AND p.cmd = 'ALL' AND p.permissive = 'PERMISSIVE' \
                  AND p.qual LIKE '%= current_setting(''runtara.tenant_id''::text, true)) \
                       OR ( SELECT runtara_all_tenants() AS runtara_all_tenants))' \
                  AND p.with_check = p.qual) \
         ORDER BY 1",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(loose.is_empty(), "tables policed otherwise: {loose:?}");

    // The function that showed a session naming no tenant every row is gone,
    // so no later policy can ask it.
    let old: i64 =
        sqlx::query_scalar("SELECT count(*) FROM pg_proc WHERE proname = 'runtara_tenant_visible'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(old, 0);

    drop_database(pool, &name).await;
}

#[tokio::test]
async fn an_environment_table_refuses_work_in_no_scope() {
    let (admin, name) = fresh().await;
    let tag = Uuid::new_v4().simple().to_string();
    let (a, b) = (format!("rls-a-{tag}"), format!("rls-b-{tag}"));
    for tenant in [&a, &b] {
        sqlx::query(
            "INSERT INTO images (image_id, tenant_id, name, binary_path) \
             VALUES ($1, $2, 'image', '/dev/null')",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(tenant)
        .execute(&admin)
        .await
        .unwrap();
    }

    // Row-level security binds neither a superuser nor a BYPASSRLS role,
    // which the test database's own may be: act as a plain role.
    let role = format!("rls_probe_{tag}");
    for statement in [
        format!("CREATE ROLE {role} NOLOGIN NOSUPERUSER NOBYPASSRLS"),
        format!("GRANT USAGE ON SCHEMA public TO {role}"),
        format!("GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO {role}"),
    ] {
        sqlx::query(&statement).execute(&admin).await.unwrap();
    }
    let options: PgConnectOptions = url().parse().unwrap();
    let pool = scoping(PgPoolOptions::new().max_connections(1).min_connections(0))
        .connect_with(options.database(&name).options([("role", role.as_str())]))
        .await
        .unwrap();
    let count = || {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM images")
                .fetch_one(&pool)
                .await
        }
    };

    assert_eq!(scoped(a.clone(), count()).await.unwrap(), 1);
    assert_eq!(system(count()).await.unwrap(), 2);
    let refused = count().await.expect_err("a read in no scope is refused");
    assert!(refused.to_string().contains("no tenant scope"), "{refused}");

    pool.close().await;
    for statement in [format!("DROP OWNED BY {role}"), format!("DROP ROLE {role}")] {
        sqlx::query(&statement).execute(&admin).await.unwrap();
    }
    drop_database(admin, &name).await;
}
