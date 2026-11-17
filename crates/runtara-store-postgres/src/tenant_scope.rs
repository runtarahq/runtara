// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Whose rows a piece of work may reach, on a pool every tenant shares (C11).
//!
//! Work runs in one of two scopes. Inside [`scoped`], each connection the pool
//! hands out names one tenant (`runtara.tenant_id`), and row-level security
//! confines the work to that tenant's rows. Inside [`system`], each names
//! every tenant (`runtara.all_tenants`): the platform's own work, such as a
//! worker that sweeps all of them, or the lookup that finds which tenant an
//! unauthenticated request is for.
//!
//! Work in neither scope reaches no tenant's rows: the database refuses its
//! reads and writes of a tenant-bearing table, so a path that forgot its scope
//! fails where it runs instead of reading across tenants. (A superuser or a
//! `BYPASSRLS` role is bound by none of this; see [`require_row_security`].)
//!
//! The scope is set on the connection as it is handed out, whether it was
//! idle (`before_acquire`) or newly opened (`after_connect`), so every query
//! and transaction on it runs under the scope it was acquired in. That costs
//! one statement per acquire, and no call site changes. The statement also
//! proves an idle connection alive (a dead one is closed and replaced), so the
//! pool's own ping before each acquire is turned off: one round trip, not two.
//!
//! The scope is a task-local: work spawned onto another task
//! (`tokio::spawn`, `spawn_blocking`) leaves it, and is in no scope unless it
//! is given one ([`inherit`], [`scoped`], [`system`]).

use sqlx::postgres::PgPoolOptions;

use runtara_tenant_scope::Named;
pub use runtara_tenant_scope::{current, inherit, is_system, locating, scoped, spawn, system};

/// Whether `pool`'s role escapes row level security (a superuser or a
/// BYPASSRLS role), so a tenant's scope would not confine it.
pub async fn escapes_row_security(pool: &sqlx::PgPool) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT rolsuper OR rolbypassrls FROM pg_roles WHERE rolname = current_user")
        .fetch_one(pool)
        .await
}

/// With `RUNTARA_REQUIRE_ROW_SECURITY` true, refuses a pool whose role
/// escapes row level security: a process that serves several tenants from
/// shared databases relies on it to keep each to its own rows. `database`
/// names the pool in the error.
pub async fn require_row_security(pool: &sqlx::PgPool, database: &str) -> Result<(), String> {
    let required = std::env::var("RUNTARA_REQUIRE_ROW_SECURITY")
        .is_ok_and(|value| matches!(value.trim(), "1" | "true" | "TRUE" | "yes"));
    if !required {
        return Ok(());
    }
    match escapes_row_security(pool).await {
        Ok(false) => Ok(()),
        Ok(true) => Err(format!(
            "RUNTARA_REQUIRE_ROW_SECURITY: the {database} database's role is a superuser or \
             BYPASSRLS, so row level security would not bind it; connect as a plain role"
        )),
        Err(error) => Err(format!(
            "RUNTARA_REQUIRE_ROW_SECURITY: could not read the {database} database's role: {error}"
        )),
    }
}

/// Warns when row level security keeps `pool`'s enum columns off their
/// indexes. Under a row security policy Postgres only pushes a condition into
/// an index scan when its operator is leakproof, and enum equality is not by
/// default: every `status = ...` on a table with a policy then scans the whole
/// table (the admission gate's count took 70 ms instead of 1 ms on 120k
/// instances). Enum equality compares two OIDs and cannot leak anything; an
/// administrator marks it once per database:
/// `ALTER FUNCTION pg_catalog.enum_eq(anyenum, anyenum) LEAKPROOF;`
pub async fn check_enum_index_use(pool: &sqlx::PgPool, database: &str) {
    let leakproof: Result<bool, sqlx::Error> = sqlx::query_scalar(
        "SELECT proleakproof FROM pg_proc WHERE oid = 'pg_catalog.enum_eq(anyenum, anyenum)'::regprocedure",
    )
    .fetch_one(pool)
    .await;
    match (escapes_row_security(pool).await, leakproof) {
        (Ok(false), Ok(false)) => tracing::warn!(
            database,
            "enum equality is not leakproof: under row level security every enum \
             condition (instance status, ...) scans its whole table. As an \
             administrator, run once in this database: ALTER FUNCTION \
             pg_catalog.enum_eq(anyenum, anyenum) LEAKPROOF;"
        ),
        (_, Err(error)) => {
            tracing::debug!(database, %error, "could not check enum equality's leakproofness")
        }
        _ => {}
    }
}

/// What work in no scope is given, from `RUNTARA_ROW_SECURITY_UNSCOPED`:
/// nothing (`refuse`, the default), or every tenant's rows with a warning
/// that says where it ran (`system`), for finding the paths that have no
/// scope yet before refusing them.
fn unscoped_is_system() -> bool {
    static MODE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *MODE.get_or_init(
        || match std::env::var("RUNTARA_ROW_SECURITY_UNSCOPED").as_deref() {
            Ok("system") => true,
            Ok("refuse") | Err(_) => false,
            Ok(other) => {
                tracing::warn!(
                    value = other,
                    "RUNTARA_ROW_SECURITY_UNSCOPED is `refuse` or `system`; refusing"
                );
                false
            }
        },
    )
}

/// Says once per place that work there took a connection in no scope.
fn warn_unscoped() {
    static SEEN: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
        std::sync::OnceLock::new();
    let trace = std::backtrace::Backtrace::force_capture().to_string();
    // The frames above the pool's own, which are the same for every caller.
    let place: String = trace
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("at ") && line.contains("/crates/"))
        .filter(|line| !line.contains("tenant_scope.rs"))
        .take(6)
        .collect::<Vec<_>>()
        .join(" < ");
    let seen = SEEN.get_or_init(Default::default);
    let new = seen
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(place.clone());
    if new {
        tracing::warn!(
            place,
            "a database connection was taken in no tenant scope; it is given every \
             tenant's rows because RUNTARA_ROW_SECURITY_UNSCOPED=system, and is refused \
             otherwise"
        );
    }
}

/// Names the current scope on `connection`: its tenant, or every tenant, or
/// neither.
async fn name_scope(connection: &mut sqlx::PgConnection) -> Result<(), sqlx::Error> {
    let (tenant, all_tenants) = match runtara_tenant_scope::named() {
        Named::Tenant(tenant) => (tenant, false),
        Named::System => (String::new(), true),
        Named::Nobody if unscoped_is_system() => {
            warn_unscoped();
            (String::new(), true)
        }
        Named::Nobody => (String::new(), false),
    };
    sqlx::query(
        "SELECT set_config('runtara.tenant_id', $1, false), \
                set_config('runtara.all_tenants', $2, false)",
    )
    .bind(tenant)
    .bind(if all_tenants { "on" } else { "off" })
    .execute(connection)
    .await
    .map(|_| ())
}

/// `options` for a pool whose connections take the scope they are handed out
/// in ([`scoped`], [`system`]), or none.
pub fn scoping(options: PgPoolOptions) -> PgPoolOptions {
    options
        .test_before_acquire(false)
        .after_connect(|connection, _| Box::pin(name_scope(connection)))
        .before_acquire(|connection, _| {
            Box::pin(async move { name_scope(connection).await.map(|_| true) })
        })
}

#[cfg(all(test, feature = "db-integration-tests"))]
mod tests {
    use super::*;
    use crate::PostgresPersistence;
    use runtara_core::persistence::Persistence;
    use uuid::Uuid;

    /// `runtara_system` is a role of the whole cluster: while the test that
    /// makes it runs, no other may be working across tenants as a plain role.
    static ROLES: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// One connection, so every scope reuses it: the tenant it names is the
    /// current scope's, never one left behind by the last.
    #[tokio::test]
    async fn a_shared_pool_serves_each_scope_its_own_tenant() {
        let _roles = ROLES.lock().await;
        let url = std::env::var("TEST_RUNTARA_DATABASE_URL")
            .expect("db-integration-tests requires TEST_RUNTARA_DATABASE_URL");
        let admin = sqlx::PgPool::connect(&url).await.unwrap();
        crate::migrations::POSTGRES
            .run(&admin)
            .await
            .expect("migrations");
        let as_admin = PostgresPersistence::new(admin.clone());
        let tag = Uuid::new_v4().simple().to_string();
        let (a, b) = (format!("scope-a-{tag}"), format!("scope-b-{tag}"));
        let (ia, ib) = (format!("scope-ia-{tag}"), format!("scope-ib-{tag}"));
        as_admin.register_instance(&ia, &a).await.unwrap();
        as_admin.register_instance(&ib, &b).await.unwrap();

        // Row-level security binds neither a superuser nor a BYPASSRLS role,
        // which the test database's own may be: act as a plain role.
        let role = format!("scope_probe_{tag}");
        for statement in [
            format!("CREATE ROLE {role} NOLOGIN NOSUPERUSER NOBYPASSRLS"),
            format!("GRANT USAGE ON SCHEMA public TO {role}"),
            format!(
                "GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO {role}"
            ),
            format!("GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO {role}"),
        ] {
            sqlx::query(&statement).execute(&admin).await.unwrap();
        }
        let options: sqlx::postgres::PgConnectOptions = url.parse().unwrap();
        let pool = scoping(PgPoolOptions::new().max_connections(1).min_connections(0))
            .connect_with(options.options([("role", role.as_str())]))
            .await
            .unwrap();
        let shared = PostgresPersistence::new(pool.clone());

        let sees = |tenant: Option<&str>, instance: &str| {
            let (shared, instance, tenant) = (
                shared.clone(),
                instance.to_owned(),
                tenant.map(str::to_owned),
            );
            async move {
                let read = async { shared.get_instance(&instance).await.unwrap().is_some() };
                match tenant {
                    Some(tenant) => scoped(tenant, read).await,
                    None => system(read).await,
                }
            }
        };
        assert!(sees(Some(&a), &ia).await);
        assert!(!sees(Some(&a), &ib).await, "another tenant's row is absent");
        assert!(
            sees(None, &ib).await,
            "the platform's scope sees every tenant"
        );
        assert!(sees(None, &ia).await);
        assert!(
            sees(Some(&b), &ib).await,
            "the next scope's tenant, not the last's"
        );
        assert!(!sees(Some(&b), &ia).await);
        // A transaction inside a scope runs under it too.
        let seen: i64 = scoped(&a, async {
            let mut tx = pool.begin().await.unwrap();
            let seen =
                sqlx::query_scalar("SELECT count(*) FROM instances WHERE instance_id IN ($1, $2)")
                    .bind(&ia)
                    .bind(&ib)
                    .fetch_one(&mut *tx)
                    .await
                    .unwrap();
            tx.commit().await.unwrap();
            seen
        })
        .await;
        assert_eq!(seen, 1);

        // In no scope, a tenant's rows are neither read nor written: the
        // statement is refused, not answered with nothing.
        let unscoped_read = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM instances WHERE instance_id IN ($1, $2)",
        )
        .bind(&ia)
        .bind(&ib)
        .fetch_one(&pool)
        .await
        .expect_err("a read in no scope is refused");
        assert!(
            unscoped_read.to_string().contains("no tenant scope"),
            "{unscoped_read}"
        );
        let unscoped_write =
            sqlx::query("UPDATE instances SET tenant_id = tenant_id WHERE instance_id = $1")
                .bind(&ia)
                .execute(&pool)
                .await
                .expect_err("a write in no scope is refused");
        assert!(
            unscoped_write.to_string().contains("no tenant scope"),
            "{unscoped_write}"
        );
        // The refusal leaves the connection usable for the next scope.
        assert!(sees(Some(&a), &ia).await);

        pool.close().await;
        for statement in [format!("DROP OWNED BY {role}"), format!("DROP ROLE {role}")] {
            sqlx::query(&statement).execute(&admin).await.unwrap();
        }
        for instance in [&ia, &ib] {
            sqlx::query("DELETE FROM instances WHERE instance_id = $1")
                .bind(instance)
                .execute(&admin)
                .await
                .unwrap();
        }
    }

    /// Until a role named `runtara_system` exists, any role may work across
    /// tenants. Once it does, only its members may: the database, not the
    /// process, decides who reaches every tenant's rows.
    #[tokio::test]
    async fn only_members_of_runtara_system_work_across_tenants_once_it_exists() {
        let _roles = ROLES.lock().await;
        let url = std::env::var("TEST_RUNTARA_DATABASE_URL")
            .expect("db-integration-tests requires TEST_RUNTARA_DATABASE_URL");
        let admin = sqlx::PgPool::connect(&url).await.unwrap();
        crate::migrations::POSTGRES
            .run(&admin)
            .await
            .expect("migrations");
        let existed: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'runtara_system')",
        )
        .fetch_one(&admin)
        .await
        .unwrap();
        assert!(
            !existed,
            "this test makes and drops the role `runtara_system`; the test cluster already has one"
        );
        let tag = Uuid::new_v4().simple().to_string();
        let tenant = format!("member-t-{tag}");
        let instance = format!("member-i-{tag}");
        PostgresPersistence::new(admin.clone())
            .register_instance(&instance, &tenant)
            .await
            .unwrap();
        let role = format!("member_probe_{tag}");
        for statement in [
            format!("CREATE ROLE {role} NOLOGIN NOSUPERUSER NOBYPASSRLS"),
            format!("GRANT USAGE ON SCHEMA public TO {role}"),
            format!("GRANT SELECT ON ALL TABLES IN SCHEMA public TO {role}"),
        ] {
            sqlx::query(&statement).execute(&admin).await.unwrap();
        }
        let options: sqlx::postgres::PgConnectOptions = url.parse().unwrap();
        let pool = scoping(PgPoolOptions::new().max_connections(1).min_connections(0))
            .connect_with(options.options([("role", role.as_str())]))
            .await
            .unwrap();
        let across = || {
            let (pool, instance) = (pool.clone(), instance.clone());
            system(async move {
                sqlx::query_scalar::<_, i64>(
                    "SELECT count(*) FROM instances WHERE instance_id = $1",
                )
                .bind(&instance)
                .fetch_one(&pool)
                .await
            })
        };
        let own = || {
            let (pool, instance) = (pool.clone(), instance.clone());
            scoped(tenant.clone(), async move {
                sqlx::query_scalar::<_, i64>(
                    "SELECT count(*) FROM instances WHERE instance_id = $1",
                )
                .bind(&instance)
                .fetch_one(&pool)
                .await
            })
        };

        let before = across().await;
        sqlx::query("CREATE ROLE runtara_system NOLOGIN")
            .execute(&admin)
            .await
            .unwrap();
        // Every check runs before the role is dropped again, whatever it finds.
        let outsider = across().await;
        let outsider_own = own().await;
        sqlx::query(&format!("GRANT runtara_system TO {role}"))
            .execute(&admin)
            .await
            .unwrap();
        let member = across().await;
        pool.close().await;
        for statement in [
            format!("DROP OWNED BY {role}"),
            format!("DROP ROLE {role}"),
            "DROP ROLE runtara_system".to_owned(),
        ] {
            sqlx::query(&statement).execute(&admin).await.unwrap();
        }
        sqlx::query("DELETE FROM instances WHERE instance_id = $1")
            .bind(&instance)
            .execute(&admin)
            .await
            .unwrap();

        assert_eq!(before.unwrap(), 1, "with no such role, any role may");
        let refused = outsider.expect_err("a role outside runtara_system may not");
        assert!(
            refused.to_string().contains("may not work across tenants"),
            "{refused}"
        );
        assert_eq!(
            outsider_own.unwrap(),
            1,
            "it still reaches a tenant it names"
        );
        assert_eq!(member.unwrap(), 1, "a member may");
    }
}
