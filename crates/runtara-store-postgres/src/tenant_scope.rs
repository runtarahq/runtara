// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Work for one tenant on a pool every tenant shares (C11): inside
//! [`scoped`], each connection the pool hands out names the tenant
//! (`runtara.tenant_id`), so row-level security confines that work to the
//! tenant's rows, while outside a scope the same pool is the system path and
//! sees every tenant.
//!
//! The tenant is set on the connection as it is handed out, whether it was
//! idle (`before_acquire`) or newly opened (`after_connect`), so every query
//! and transaction on it runs under the scope it was acquired in. That costs
//! one statement per acquire, and no call site changes.
//!
//! The scope is a task-local: work spawned onto another task
//! (`tokio::spawn`, `spawn_blocking`) leaves it and runs as the system path.

use std::future::Future;

use sqlx::postgres::PgPoolOptions;

tokio::task_local! {
    static TENANT: String;
}

/// Runs `work` for `tenant`: connections it takes from a pool built with
/// [`scoping`] see only that tenant's rows.
pub async fn scoped<F: Future>(tenant: impl Into<String>, work: F) -> F::Output {
    TENANT.scope(tenant.into(), work).await
}

/// The tenant of the current scope, if any.
pub fn current() -> Option<String> {
    TENANT.try_with(Clone::clone).ok()
}

async fn name_tenant(connection: &mut sqlx::PgConnection) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT set_config('runtara.tenant_id', $1, false)")
        .bind(current().unwrap_or_default())
        .execute(connection)
        .await
        .map(|_| ())
}

/// `options` for a pool whose connections take the tenant of the scope they
/// are handed out in ([`scoped`]), or none.
pub fn scoping(options: PgPoolOptions) -> PgPoolOptions {
    options
        .after_connect(|connection, _| Box::pin(name_tenant(connection)))
        .before_acquire(|connection, _| {
            Box::pin(async move { name_tenant(connection).await.map(|_| true) })
        })
}

#[cfg(all(test, feature = "db-integration-tests"))]
mod tests {
    use super::*;
    use crate::PostgresPersistence;
    use runtara_core::persistence::Persistence;
    use uuid::Uuid;

    /// One connection, so every scope reuses it: the tenant it names is the
    /// current scope's, never one left behind by the last.
    #[tokio::test]
    async fn a_shared_pool_serves_each_scope_its_own_tenant() {
        let url = std::env::var("TEST_RUNTARA_DATABASE_URL")
            .expect("db-integration-tests requires TEST_RUNTARA_DATABASE_URL");
        let admin = sqlx::PgPool::connect(&url).await.unwrap();
        crate::migrations::POSTGRES
            .run(&admin)
            .await
            .expect("migrations");
        let system = PostgresPersistence::new(admin.clone());
        let tag = Uuid::new_v4().simple().to_string();
        let (a, b) = (format!("scope-a-{tag}"), format!("scope-b-{tag}"));
        let (ia, ib) = (format!("scope-ia-{tag}"), format!("scope-ib-{tag}"));
        system.register_instance(&ia, &a).await.unwrap();
        system.register_instance(&ib, &b).await.unwrap();

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
                    None => read.await,
                }
            }
        };
        assert!(sees(Some(&a), &ia).await);
        assert!(!sees(Some(&a), &ib).await, "another tenant's row is absent");
        assert!(sees(None, &ib).await, "the system path sees every tenant");
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
}
