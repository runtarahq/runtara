// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Postgres permission boundary for raw SQL, as provisioned by the
//! compose/install init script: a non-superuser object-model login role that
//! owns only its database and has no CONNECT on the server database.
//!
//! Requires the explicit `db-integration-tests` feature and
//! `TEST_RUNTARA_SERVER_DATABASE_URL` connecting as a role that may create
//! roles and databases. Every role and database this creates has a unique
//! name and is dropped at the end. Own binary: the server config is
//! process-global.

use std::sync::Arc;

use futures::FutureExt;
use runtara_connections::crypto::noop::NoOpCipher;
use runtara_connections::{
    ConnectionsConfig, ConnectionsFacade, ConnectionsState, IntegrationCompatibility,
};
use runtara_server::api::dto::object_model::{
    SqlExecuteRequest, SqlQueryRequest, SqlRawQueryRequest,
};
use runtara_server::api::repositories::object_model::ObjectStoreManager;
use runtara_server::api::services::object_model::{InstanceService, ServiceError};
use runtara_server::object_model_privileges as privileges;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

const TENANT: &str = "sql_roles_test";

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

fn admin_url() -> String {
    std::env::var("TEST_RUNTARA_SERVER_DATABASE_URL")
        .or_else(|_| std::env::var("RUNTARA_SERVER_DATABASE_URL"))
        .expect("db-integration-tests requires TEST_RUNTARA_SERVER_DATABASE_URL")
}

/// `admin_url` with a different login and database.
fn url_as(user: &str, password: &str, database: &str) -> String {
    let mut url = url::Url::parse(&admin_url()).expect("admin URL parses");
    url.set_username(user).unwrap();
    url.set_password(Some(password)).unwrap();
    url.set_path(database);
    url.to_string()
}

struct Names {
    objects_role: String,
    objects_db: String,
    server_db: String,
    password: String,
}

impl Names {
    fn new() -> Self {
        let id = &Uuid::new_v4().simple().to_string()[..12];
        Self {
            objects_role: format!("rtsql_{id}_objects"),
            objects_db: format!("rtsql_{id}_objects"),
            server_db: format!("rtsql_{id}_server"),
            password: Uuid::new_v4().simple().to_string(),
        }
    }
}

async fn exec(pool: &PgPool, sql: &str) {
    sqlx::query(sql)
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("{sql}: {error}"));
}

/// The role statements from the compose/install init script, with this test's
/// unique names. The cluster-level part runs on the admin database.
async fn provision(admin: &PgPool, n: &Names) {
    let objects = &n.objects_role;
    for sql in [
        format!(
            "CREATE ROLE {objects} LOGIN PASSWORD '{}' \
             NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS",
            n.password
        ),
        format!("CREATE DATABASE {}", n.server_db),
        format!("CREATE DATABASE {} OWNER {objects}", n.objects_db),
        format!(
            "REVOKE CONNECT, TEMPORARY ON DATABASE {} FROM PUBLIC",
            n.server_db
        ),
    ] {
        exec(admin, &sql).await;
    }

    // Per-database part, as the init script's `\c runtara_objects` block.
    let objects_admin = PgPool::connect(&{
        let mut url = url::Url::parse(&admin_url()).unwrap();
        url.set_path(&n.objects_db);
        url.to_string()
    })
    .await
    .unwrap();
    for sql in [
        format!("ALTER SCHEMA public OWNER TO {objects}"),
        "REVOKE ALL ON SCHEMA public FROM PUBLIC".to_string(),
    ] {
        exec(&objects_admin, &sql).await;
    }
    objects_admin.close().await;
}

async fn teardown(admin: &PgPool, n: &Names) {
    for sql in [
        format!("DROP DATABASE IF EXISTS {} WITH (FORCE)", n.objects_db),
        format!("DROP DATABASE IF EXISTS {} WITH (FORCE)", n.server_db),
        format!("DROP ROLE IF EXISTS {}", n.objects_role),
    ] {
        if let Err(error) = sqlx::query(&sql).execute(admin).await {
            eprintln!("cleanup failed: {sql}: {error}");
        }
    }
}

fn service(admin: &PgPool) -> InstanceService {
    let connections = Arc::new(ConnectionsFacade::new(ConnectionsState::from_config(
        ConnectionsConfig {
            db_pool: admin.clone(),
            redis_manager: None,
            public_base_url: "http://localhost".to_string(),
            http_client: reqwest::Client::new(),
            cipher: Arc::new(NoOpCipher),
            compatibility: Arc::new(IntegrationCompatibility::new(Default::default())),
            agent_catalog: Arc::new(runtara_dsl::agent_meta::AgentCatalog::from_agents(
                Vec::new(),
            )),
            connection_events: None,
        },
    )));
    InstanceService::new(
        Arc::new(ObjectStoreManager::new(
            runtara_server::config::object_model_database_url(),
        )),
        connections,
    )
}

fn raw(sql: &str) -> SqlRawQueryRequest {
    serde_json::from_value(json!({ "sql": sql })).unwrap()
}

fn typed(sql: &str, column: &str) -> SqlQueryRequest {
    serde_json::from_value(json!({
        "sql": sql,
        "resultSchema": [{ "name": column, "type": "string" }],
    }))
    .unwrap()
}

fn command(sql: &str) -> SqlExecuteRequest {
    serde_json::from_value(json!({ "sql": sql })).unwrap()
}

fn validation_message(result: Result<impl std::fmt::Debug, ServiceError>) -> String {
    match result {
        Err(ServiceError::ValidationError(message)) => message,
        other => panic!("expected a 400-class validation error, got {other:?}"),
    }
}

async fn assertions(admin: &PgPool, n: &Names) {
    let svc = service(admin);
    let objects_pool = PgPool::connect(&runtara_server::config::object_model_database_url())
        .await
        .unwrap();

    // The boot check sees a non-superuser role on its own database.
    let facts = privileges::collect_facts(&objects_pool, admin)
        .await
        .unwrap();
    assert!(
        !facts.superuser,
        "object-model login role is not a superuser"
    );
    assert!(!facts.shares_server_database);
    assert!(privileges::privilege_warnings(facts).is_empty());

    // sql/execute runs as the object-model role and manages its own tables.
    for sql in [
        "CREATE TABLE items (id BIGINT PRIMARY KEY, name TEXT NOT NULL)",
        "INSERT INTO items VALUES (1, 'alpha'), (2, 'beta')",
    ] {
        svc.execute_sql(TENANT, command(sql), None)
            .await
            .unwrap_or_else(|error| panic!("{sql}: {error}"));
    }
    let affected = svc
        .execute_sql(
            TENANT,
            command("UPDATE items SET name = 'gamma' WHERE id = 2"),
            None,
        )
        .await
        .unwrap();
    assert_eq!(affected, 1);

    // Read routes read the object tables as that role, and never write.
    let who = svc
        .query_sql_raw(TENANT, raw("SELECT current_user::text AS who"), None)
        .await
        .unwrap();
    assert_eq!(who, vec![json!({ "who": n.objects_role })]);
    let rows = svc
        .query_sql(
            TENANT,
            typed("SELECT name FROM items ORDER BY id", "name"),
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        rows,
        vec![json!({"name": "alpha"}), json!({"name": "gamma"})]
    );
    let write = validation_message(
        svc.query_sql_raw(
            TENANT,
            raw("WITH d AS (DELETE FROM items RETURNING 1) SELECT count(*) AS n FROM d"),
            None,
        )
        .await,
    );
    assert!(write.contains("25006"), "{write}");
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM items")
        .fetch_one(&objects_pool)
        .await
        .unwrap();
    assert_eq!(count, 2, "no read route wrote");

    // The object-model role cannot reach the server database or escape Postgres.
    let server_as_objects = url_as(&n.objects_role, &n.password, &n.server_db);
    let error = PgPool::connect(&server_as_objects)
        .await
        .expect_err("object-model role must not connect to the server database");
    let sqlstate = error
        .as_database_error()
        .and_then(|db| db.code())
        .map(|code| code.into_owned());
    assert_eq!(sqlstate.as_deref(), Some("42501"), "{error}");

    for sql in [
        "CREATE EXTENSION dblink",
        "CREATE EXTENSION postgres_fdw",
        "COPY (SELECT 1) TO PROGRAM 'true'",
        "COPY (SELECT 1) TO '/tmp/rtsql_escape'",
    ] {
        let result = svc.execute_sql(TENANT, command(sql), None).await;
        let message = format!("{result:?}");
        assert!(result.is_err(), "{sql} must be denied");
        assert!(
            message.contains("42501") || message.contains("permission denied"),
            "{sql}: {message}"
        );
    }

    objects_pool.close().await;
}

#[tokio::test]
async fn object_model_role_bounds_raw_sql() {
    let admin = PgPool::connect(&admin_url())
        .await
        .expect("required server test database must accept connections");
    MIGRATOR
        .run(&admin)
        .await
        .expect("required server migrations must succeed");

    let names = Names::new();
    // SAFETY: the only test in this binary; set before the config is read.
    unsafe {
        std::env::set_var("TENANT_ID", TENANT);
        std::env::set_var(
            "OBJECT_MODEL_DATABASE_URL",
            url_as(&names.objects_role, &names.password, &names.objects_db),
        );
        std::env::set_var("RUNTARA_MCP_SESSION_STORE", "local");
    }
    runtara_server::config::init(
        runtara_server::config::Config::from_env().expect("SQL roles test config"),
    );

    provision(&admin, &names).await;
    let outcome = std::panic::AssertUnwindSafe(assertions(&admin, &names))
        .catch_unwind()
        .await;
    teardown(&admin, &names).await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}
