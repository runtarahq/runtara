// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Shared Postgres fixtures for this crate's database-backed unit tests.
//!
//! The unit tests in [`crate::runtime_host`], [`crate::runner::embedded`] and
//! [`crate::http_server`] do not merely orchestrate: they assert what the
//! database is left holding after a write — that an acknowledged signal stops
//! being pending, that `complete_instance` COALESCEs termination fields, that a
//! terminal status survives a late suspend. Those are properties of the real
//! schema and the real SQL, so they run against a real Postgres rather than a
//! mock that would only confirm its own behaviour.
//!
//! Every fixture here mints a fresh instance id. The database is shared across
//! the whole run — and across runs — and `register_instance` is a bare INSERT
//! with no `ON CONFLICT`, so a fixed id would collide with its own previous
//! run rather than start clean.

use std::sync::Arc;

use runtara_core::persistence::{Persistence, PostgresPersistence};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{Connection, PgConnection, PgPool};
use uuid::Uuid;

/// Connect to this crate's unit-test database, bringing its schema up to date.
///
/// Deliberately a *different* database from the one `tests/` uses, derived by
/// suffixing the configured name. Several integration tests drive machinery that
/// queries the whole database rather than one tenant — `WakeScheduler` claims
/// every instance whose `sleep_until` is due, in batches of ten — and the unit
/// tests here park instances that have no image row. Sharing one database lets
/// those parked rows clog the scheduler's batch with launches that can never
/// succeed, and the integration test fails for a reason that has nothing to do
/// with what it asserts.
///
/// A fresh pool per call, also deliberately. A `PgPool` belongs to the tokio
/// runtime that built it — its connection reaper is a task on that runtime — and
/// every `#[tokio::test]` gets its own. Caching one pool in a static hands the
/// second test a pool nothing is servicing, which surfaces as "pool timed out
/// while waiting for an open connection" rather than anything that names the
/// real problem. Re-running the migrator each time is close to free: after the
/// first call it reads the ledger and applies nothing.
pub(crate) async fn pool() -> PgPool {
    let url = std::env::var("TEST_ENVIRONMENT_DATABASE_URL")
        .or_else(|_| std::env::var("RUNTARA_ENVIRONMENT_DATABASE_URL"))
        .expect(
            "db-integration-tests requires TEST_ENVIRONMENT_DATABASE_URL \
             or RUNTARA_ENVIRONMENT_DATABASE_URL",
        );
    let base: PgConnectOptions = url
        .parse()
        .expect("the environment test database URL must parse");
    let unit_db = format!("{}_unit", base.get_database().unwrap_or("runtara_test"));

    // Create it on first use so a developer only has to provision the one
    // database the suite already documents.
    let admin = base.clone().database("postgres");
    if let Ok(mut conn) = PgConnection::connect_with(&admin).await {
        let exists: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM pg_database WHERE datname = $1")
            .bind(&unit_db)
            .fetch_optional(&mut conn)
            .await
            .unwrap_or(None);
        if exists.is_none() {
            // A concurrent test binary may win this race; a duplicate-database
            // error here is success by another route.
            let _ = sqlx::query(&format!("CREATE DATABASE \"{unit_db}\""))
                .execute(&mut conn)
                .await;
        }
    }

    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect_with(base.database(&unit_db))
        .await
        .expect("required environment unit-test database must accept connections");
    crate::migrations::run(&pool)
        .await
        .expect("required combined core/environment migrations must succeed");
    pool
}

/// A `Persistence` backed by the unit-test database.
pub(crate) async fn persistence() -> Arc<dyn Persistence> {
    Arc::new(PostgresPersistence::new(pool().await))
}

/// An instance id no other test — or earlier run — can be holding.
pub(crate) fn unique_id(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4())
}

/// A registered instance already moved to `running`, the state the environment
/// establishes before a launch.
pub(crate) async fn running_instance(prefix: &str) -> (Arc<dyn Persistence>, String) {
    let persistence = persistence().await;
    let instance_id = unique_id(prefix);
    persistence
        .register_instance(&instance_id, &format!("{prefix}-tenant"))
        .await
        .expect("register instance");
    persistence
        .update_instance_status(&instance_id, "running", None)
        .await
        .expect("mark running");
    (persistence, instance_id)
}
