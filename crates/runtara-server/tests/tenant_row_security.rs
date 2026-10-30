// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Row-level security on the server database (M7.11): every table is policed
//! by tenant unless it is global by design, and a connection scoped to one
//! tenant neither reads nor writes another's rows.
//!
//! Requires the explicit `db-integration-tests` feature and a live Postgres.

use runtara_store_postgres::tenant_scope::{scoped, scoping};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Tables with no tenant: the migration record and the process's own
/// plan-check record. Anything else must be policed.
const GLOBAL: &[&str] = &["_sqlx_migrations", "metadata"];

fn url() -> String {
    std::env::var("TEST_RUNTARA_SERVER_DATABASE_URL")
        .or_else(|_| std::env::var("RUNTARA_SERVER_DATABASE_URL"))
        .expect("db-integration-tests requires TEST_RUNTARA_SERVER_DATABASE_URL")
}

async fn admin() -> PgPool {
    let pool = PgPool::connect(&url())
        .await
        .expect("required server test database must accept connections");
    MIGRATOR
        .run(&pool)
        .await
        .expect("required server migrations must succeed");
    pool
}

/// A pool scoped as the server's is, connecting as a plain role (neither
/// superuser nor BYPASSRLS, which the test database's own may be).
struct Restricted {
    admin: PgPool,
    role: String,
    pool: PgPool,
}

impl Restricted {
    async fn new(admin: &PgPool) -> Self {
        let role = format!("rls_probe_{}", Uuid::new_v4().simple());
        for statement in [
            format!("CREATE ROLE {role} NOLOGIN NOSUPERUSER NOBYPASSRLS"),
            format!("GRANT USAGE ON SCHEMA public TO {role}"),
            format!(
                "GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO {role}"
            ),
            format!("GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO {role}"),
        ] {
            sqlx::query(&statement).execute(admin).await.unwrap();
        }
        let options: PgConnectOptions = url().parse().unwrap();
        let pool = scoping(PgPoolOptions::new().max_connections(1).min_connections(0))
            .connect_with(options.options([("role", role.as_str())]))
            .await
            .unwrap();
        Self {
            admin: admin.clone(),
            role,
            pool,
        }
    }

    async fn drop(self) {
        self.pool.close().await;
        for statement in [
            format!("DROP OWNED BY {}", self.role),
            format!("DROP ROLE {}", self.role),
        ] {
            sqlx::query(&statement).execute(&self.admin).await.unwrap();
        }
    }
}

/// A database of its own, migrated: the shared test database also holds
/// tables other tests make (object-model tables, for one).
async fn fresh() -> (PgPool, String) {
    let admin = PgPool::connect(&url()).await.unwrap();
    let name = format!("rls_guard_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE {name}"))
        .execute(&admin)
        .await
        .unwrap();
    let options: PgConnectOptions = url().parse().unwrap();
    let pool = PgPool::connect_with(options.database(&name)).await.unwrap();
    MIGRATOR.run(&pool).await.expect("migrations");
    (pool, name)
}

#[tokio::test]
async fn every_table_is_policed_by_tenant_or_global_by_design() {
    let (pool, name) = fresh().await;
    let unpoliced: Vec<String> = sqlx::query_scalar(
        "SELECT c.relname::text FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = 'public' AND c.relkind IN ('r', 'p') \
           AND NOT (c.relrowsecurity AND c.relforcerowsecurity) \
         ORDER BY 1",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let unexpected: Vec<&String> = unpoliced
        .iter()
        .filter(|table| !GLOBAL.contains(&table.as_str()))
        .collect();
    assert!(
        unexpected.is_empty(),
        "tables without forced row-level security: {unexpected:?}; police them by tenant \
         (as migration 20261030000000 does) or add them to GLOBAL with the reason"
    );

    // Each policed table checks reads and writes alike, through the one
    // tenant predicate.
    let loose: Vec<String> = sqlx::query_scalar(
        "SELECT c.relname::text FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = 'public' AND c.relkind IN ('r', 'p') AND c.relforcerowsecurity \
           AND NOT EXISTS (SELECT 1 FROM pg_policies p \
                WHERE p.schemaname = 'public' AND p.tablename = c.relname \
                  AND p.cmd = 'ALL' AND p.permissive = 'PERMISSIVE' \
                  AND p.qual LIKE 'runtara_tenant_visible(%' \
                  AND p.with_check LIKE 'runtara_tenant_visible(%') \
            OR EXISTS (SELECT 1 FROM pg_policies p \
                WHERE p.schemaname = 'public' AND p.tablename = c.relname \
                  AND p.policyname <> 'runtara_tenant') \
         ORDER BY 1",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(loose.is_empty(), "tables policed otherwise: {loose:?}");

    pool.close().await;
    let admin = PgPool::connect(&url()).await.unwrap();
    sqlx::query(&format!("DROP DATABASE {name}"))
        .execute(&admin)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_scoped_connection_reaches_only_its_tenants_rows() {
    let admin = admin().await;
    let tag = Uuid::new_v4().simple().to_string();
    let (a, b) = (format!("rls-a-{tag}"), format!("rls-b-{tag}"));
    for tenant in [&a, &b] {
        sqlx::query("INSERT INTO workflows (tenant_id, workflow_id) VALUES ($1, 'wf')")
            .bind(tenant)
            .execute(&admin)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO api_keys (org_id, name, key_prefix, key_hash, issuing_user_id) \
             VALUES ($1, 'k', 'p', $2, 'u')",
        )
        .bind(tenant)
        .bind(format!("hash-{tenant}"))
        .execute(&admin)
        .await
        .unwrap();
    }
    let restricted = Restricted::new(&admin).await;
    let pool = restricted.pool.clone();
    let tenants = [a.clone(), b.clone()];

    let counted = |tenant: Option<String>, sql: &'static str| {
        let (pool, tenants) = (pool.clone(), tenants.clone());
        async move {
            let count = async {
                sqlx::query_scalar::<_, i64>(sql)
                    .bind(&tenants[..])
                    .fetch_one(&pool)
                    .await
                    .unwrap()
            };
            match tenant {
                Some(tenant) => scoped(tenant, count).await,
                None => count.await,
            }
        }
    };
    let workflows = "SELECT count(*) FROM workflows WHERE tenant_id = ANY($1)";
    let keys = "SELECT count(*) FROM api_keys WHERE org_id = ANY($1)";
    assert_eq!(counted(Some(a.clone()), workflows).await, 1);
    assert_eq!(counted(Some(a.clone()), keys).await, 1);
    assert_eq!(
        counted(None, workflows).await,
        2,
        "the system path sees all"
    );

    // Writes: another tenant's rows are untouched, and no row can be written
    // as another tenant.
    let (updated, deleted) = scoped(a.clone(), async {
        let updated = sqlx::query("UPDATE workflows SET deleted_at = now() WHERE tenant_id = $1")
            .bind(&b)
            .execute(&pool)
            .await
            .unwrap()
            .rows_affected();
        let deleted = sqlx::query("DELETE FROM api_keys WHERE org_id = $1")
            .bind(&b)
            .execute(&pool)
            .await
            .unwrap()
            .rows_affected();
        (updated, deleted)
    })
    .await;
    assert_eq!((updated, deleted), (0, 0));
    let foreign = scoped(a.clone(), async {
        sqlx::query("INSERT INTO workflows (tenant_id, workflow_id) VALUES ($1, 'planted')")
            .bind(&b)
            .execute(&pool)
            .await
    })
    .await;
    assert!(foreign.is_err(), "a row for another tenant is refused");
    let moved = scoped(a.clone(), async {
        sqlx::query("UPDATE workflows SET tenant_id = $2 WHERE tenant_id = $1")
            .bind(&a)
            .bind(&b)
            .execute(&pool)
            .await
    })
    .await;
    assert!(moved.is_err(), "a row cannot be moved to another tenant");

    restricted.drop().await;
    for tenant in [&a, &b] {
        sqlx::query("DELETE FROM workflows WHERE tenant_id = $1")
            .bind(tenant)
            .execute(&admin)
            .await
            .unwrap();
        sqlx::query("DELETE FROM api_keys WHERE org_id = $1")
            .bind(tenant)
            .execute(&admin)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn an_outbox_row_takes_its_requests_tenant() {
    let admin = admin().await;
    let tenant = format!("rls-outbox-{}", Uuid::new_v4().simple());
    let request = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO execution_requests \
           (request_id, tenant_id, idempotency_key, instance_id, workflow_id, trigger_event, deadline_at) \
         VALUES ($1, $2, $3, $3, 'wf', '{}', now() + interval '1 hour')",
    )
    .bind(request)
    .bind(&tenant)
    .bind(request.to_string())
    .execute(&admin)
    .await
    .unwrap();
    // As a release from before the tenant column writes it.
    sqlx::query("INSERT INTO execution_outbox (request_id) VALUES ($1)")
        .bind(request)
        .execute(&admin)
        .await
        .unwrap();
    let stored: String =
        sqlx::query_scalar("SELECT tenant_id FROM execution_outbox WHERE request_id = $1")
            .bind(request)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(stored, tenant);
    sqlx::query("DELETE FROM execution_requests WHERE request_id = $1")
        .bind(request)
        .execute(&admin)
        .await
        .unwrap();
}
