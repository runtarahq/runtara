// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Round-trip tests for the SYN-437 audit emitter (`audit::emit`).
//!
//! Need a live Postgres. Skip cleanly when neither `TEST_RUNTARA_SERVER_DATABASE_URL` nor
//! `RUNTARA_SERVER_DATABASE_URL` is set, mirroring `invocation_cleanup_test.rs`. Run with:
//!   `RUNTARA_SERVER_DATABASE_URL=postgres://... cargo test -p runtara-server --test audit_events`

use runtara_server::audit::{self, AuditEvent};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

macro_rules! skip_if_no_db {
    () => {
        if std::env::var("TEST_RUNTARA_SERVER_DATABASE_URL").is_err()
            && std::env::var("RUNTARA_SERVER_DATABASE_URL").is_err()
        {
            eprintln!(
                "Skipping test: TEST_RUNTARA_SERVER_DATABASE_URL or RUNTARA_SERVER_DATABASE_URL not set"
            );
            return;
        }
    };
}

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

async fn get_test_pool() -> Option<PgPool> {
    let url = std::env::var("TEST_RUNTARA_SERVER_DATABASE_URL")
        .or_else(|_| std::env::var("RUNTARA_SERVER_DATABASE_URL"))
        .ok()?;
    let pool = PgPool::connect(&url).await.ok()?;
    MIGRATOR.run(&pool).await.ok()?;
    Some(pool)
}

#[tokio::test]
async fn emit_writes_a_row_with_contract_columns() {
    skip_if_no_db!();
    let pool = get_test_pool().await.expect("test pool");
    let tenant = format!("t-{}", Uuid::new_v4());
    let resource_id = Uuid::new_v4().to_string();

    audit::emit(
        &pool,
        &tenant,
        Some("auth0|actor"),
        AuditEvent::new("token.create")
            .resource("api_key", &resource_id)
            .payload(json!({ "name": "ci" })),
    )
    .await;

    let row = sqlx::query_as::<
        _,
        (
            String,
            Option<String>,
            String,
            String,
            Option<String>,
            Option<String>,
        ),
    >(
        "SELECT tenant_id, actor_user_id, source, event_type, resource_type, resource_id \
         FROM audit_events WHERE tenant_id = $1",
    )
    .bind(&tenant)
    .fetch_one(&pool)
    .await
    .expect("audit row");

    assert_eq!(row.0, tenant);
    assert_eq!(row.1.as_deref(), Some("auth0|actor"));
    assert_eq!(row.2, "runtara");
    assert_eq!(row.3, "token.create");
    assert_eq!(row.4.as_deref(), Some("api_key"));
    assert_eq!(row.5.as_deref(), Some(resource_id.as_str()));

    let _ = sqlx::query("DELETE FROM audit_events WHERE tenant_id = $1")
        .bind(&tenant)
        .execute(&pool)
        .await;
}

#[tokio::test]
async fn emit_allows_null_actor_for_system_events() {
    skip_if_no_db!();
    let pool = get_test_pool().await.expect("test pool");
    let tenant = format!("t-{}", Uuid::new_v4());

    audit::emit(&pool, &tenant, None, AuditEvent::new("system.reconcile")).await;

    let actor: Option<String> =
        sqlx::query_scalar("SELECT actor_user_id FROM audit_events WHERE tenant_id = $1")
            .bind(&tenant)
            .fetch_one(&pool)
            .await
            .expect("audit row");
    assert_eq!(actor, None);

    let _ = sqlx::query("DELETE FROM audit_events WHERE tenant_id = $1")
        .bind(&tenant)
        .execute(&pool)
        .await;
}
