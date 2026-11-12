// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! `runtara-server object-model set`: a tenant's object model database is a
//! Postgres connection of the tenant's, entered once and stored sealed. The
//! platform supplies no database of its own.
//!
//! Requires the explicit `db-integration-tests` feature and a live Postgres.
//! Own binary: the server config is process-global.

use std::sync::Arc;

use runtara_connections::crypto::noop::NoOpCipher;
use runtara_connections::sealed::Allowances;
use runtara_connections::{
    ConnectionsConfig, ConnectionsFacade, ConnectionsState, IntegrationCompatibility,
};
use runtara_server::api::dto::object_model::SqlRawQueryRequest;
use runtara_server::api::repositories::object_model::ObjectStoreManager;
use runtara_server::api::services::object_model::InstanceService;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

fn database_url() -> url::Url {
    let url = std::env::var("TEST_RUNTARA_SERVER_DATABASE_URL")
        .or_else(|_| std::env::var("RUNTARA_SERVER_DATABASE_URL"))
        .expect("db-integration-tests requires TEST_RUNTARA_SERVER_DATABASE_URL");
    url::Url::parse(&url).expect("the test database URL parses")
}

#[tokio::test]
async fn a_tenants_object_model_database_is_entered_once_and_stored_sealed() {
    let mut url = database_url();
    let host = url
        .host_str()
        .expect("the test database has a host")
        .to_owned();
    // SAFETY: the only test in this binary; set before the config is read.
    unsafe {
        std::env::set_var("TENANT_ID", "object_model_set_test");
        std::env::set_var("RUNTARA_MCP_SESSION_STORE", "local");
        // The tenant's database is the test database: on loopback, and
        // reached with no TLS.
        std::env::set_var("RUNTARA_DATABASE_CONNECTION_GUARD", "off");
        std::env::set_var("RUNTARA_PROXY_ALLOW_HTTP_HOSTS", &host);
        if std::env::var("RUNTARA_SEAL_WRAPPING_KEY").is_err() {
            // Thirty-two zero bytes: a key for this test's database only.
            std::env::set_var(
                "RUNTARA_SEAL_WRAPPING_KEY",
                "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
            );
        }
    }
    runtara_server::config::init(
        runtara_server::config::Config::from_env().expect("object-model set test config"),
    );
    let pool = PgPool::connect(url.as_str())
        .await
        .expect("required server test database must accept connections");
    MIGRATOR
        .run(&pool)
        .await
        .expect("required server migrations must succeed");
    let tenant = format!("om_set_{}", Uuid::new_v4().simple());
    let cipher = || Some(Arc::new(NoOpCipher) as Arc<dyn runtara_connections::CredentialCipher>);
    let set = |url: String| {
        let (pool, tenant) = (pool.clone(), tenant.clone());
        async move { runtara_server::object_model::set(pool, &tenant, &url, cipher()).await }
    };

    // A URL must say how its session is protected, and be a Postgres one.
    // Neither refusal repeats it.
    url.set_query(None);
    let unprotected = url.to_string();
    for refused in [
        unprotected.clone(),
        unprotected.replace("postgres", "mysql"),
    ] {
        let error = set(refused.clone()).await.expect_err("refused");
        assert!(!error.contains(&refused), "the URL is not repeated");
        assert!(
            error.contains("sslmode") || error.contains("postgres://"),
            "{error}"
        );
    }
    let defaults = || async {
        sqlx::query_scalar::<_, String>(
            "SELECT connection_id FROM connection_defaults \
             WHERE tenant_id = $1 AND default_for = 'object_model'",
        )
        .bind(&tenant)
        .fetch_optional(&pool)
        .await
        .unwrap()
    };
    assert_eq!(defaults().await, None, "nothing was stored");

    url.set_query(Some("sslmode=disable"));
    let first = set(url.to_string()).await.expect("the database is set");
    assert_eq!(defaults().await.as_deref(), Some(first.as_str()));
    let (stored, format): (String, String) = sqlx::query_as(
        "SELECT connection_parameters::text, credential_format \
         FROM connection_data_entity WHERE id = $1 AND tenant_id = $2",
    )
    .bind(&first)
    .bind(&tenant)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(format, "seal");
    assert!(stored.contains("$sealed"), "{stored}");
    if let Some(password) = url.password() {
        assert!(
            !stored.contains(&format!(":{password}@")),
            "no readable URL"
        );
    }

    // The tenant's requests that name no connection use it, through seal.
    let sealed = runtara_connections::sealed::from_env(
        pool.clone(),
        Allowances {
            plain_http_hosts: vec![host],
            private_databases: true,
            ..Default::default()
        },
    )
    .expect("seal starts");
    let connections = Arc::new(ConnectionsFacade::new(ConnectionsState::from_config(
        ConnectionsConfig {
            sealed,
            agent_access: Arc::new(runtara_connections::EveryAgent),
            db_pool: pool.clone(),
            redis_manager: None,
            public_base_url: "http://localhost".to_string(),
            oauth_complete_url: None,
            http_client: reqwest::Client::new(),
            cipher: Arc::new(NoOpCipher),
            compatibility: Arc::new(IntegrationCompatibility::new(Default::default())),
            agent_catalog: Arc::new(runtara_dsl::agent_meta::AgentCatalog::from_agents(
                Vec::new(),
            )),
            connection_events: None,
            oauth: Default::default(),
        },
    )));
    let service = InstanceService::new(Arc::new(ObjectStoreManager::new()), connections);
    let request: SqlRawQueryRequest =
        serde_json::from_value(json!({ "sql": "SELECT current_database()::text AS db" })).unwrap();
    let rows = service
        .query_sql_raw(&tenant, request, None)
        .await
        .expect("the tenant's default database answers");
    assert_eq!(
        rows,
        vec![json!({ "db": url.path().trim_start_matches('/') })]
    );

    // Set again, the URL is replaced where it is: the same connection, with
    // nothing left holding what it held, and refused as before if it cannot
    // be sealed.
    let before = stored;
    assert!(set(unprotected).await.is_err());
    let second = set(url.to_string())
        .await
        .expect("the database is set again");
    assert_eq!(first, second);
    assert_eq!(defaults().await.as_deref(), Some(first.as_str()));
    let (after, count): (String, i64) = sqlx::query_as(
        "SELECT min(connection_parameters::text), count(*) \
         FROM connection_data_entity WHERE tenant_id = $1",
    )
    .bind(&tenant)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1, "no connection is left behind");
    assert_ne!(before, after, "sealed anew");

    sqlx::query("DELETE FROM connection_defaults WHERE tenant_id = $1")
        .bind(&tenant)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM connection_data_entity WHERE tenant_id = $1")
        .bind(&tenant)
        .execute(&pool)
        .await
        .unwrap();
}
