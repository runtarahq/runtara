//! Integration test for the OAuth refresh-token rotation persistence write path
//! (`ConnectionRepository::persist_refreshed_oauth`) and its optimistic-concurrency
//! guard, against a real Postgres. Uses synthetic generation markers ("g0", "g1", …)
//! in place of real SHA-256 hashes — the guard only does string equality, so this
//! isolates the concurrency semantics from the hashing. A reauthorization computes
//! the hash itself, so the tests that store one use the real hash.

use std::sync::Arc;

use runtara_connections::crypto::noop::NoOpCipher;
use runtara_connections::repository::connections::{ConnectionRepository, refresh_token_guard};
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::Row;
use testcontainers::ContainerAsync;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;

struct PgFixture {
    pool: PgPool,
    _container: ContainerAsync<Postgres>,
}

impl PgFixture {
    async fn start() -> Self {
        let container = Postgres::default()
            .start()
            .await
            .expect("required Docker Postgres container must start");
        let host = container.get_host().await.expect("required Postgres host");
        let port = container
            .get_host_port_ipv4(5432)
            .await
            .expect("required Postgres port");
        let url = format!("postgres://postgres:postgres@{host}:{port}/postgres");
        let pool = PgPool::connect(&url)
            .await
            .expect("required Postgres connection");
        // Minimal schema: only the columns the OAuth token writes touch.
        sqlx::query(
            r#"
            CREATE TABLE connection_data_entity (
                id VARCHAR(255) PRIMARY KEY,
                tenant_id VARCHAR(255) NOT NULL,
                connection_parameters JSONB DEFAULT NULL,
                credential_format TEXT NOT NULL DEFAULT 'cipher',
                refresh_token_hash TEXT DEFAULT NULL,
                status VARCHAR(64) DEFAULT NULL,
                updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
            "#,
        )
        .execute(&pool)
        .await
        .expect("create required connection table");
        Self {
            pool,
            _container: container,
        }
    }
}

async fn insert_conn(pool: &PgPool, id: &str, tenant: &str, params: &Value) {
    sqlx::query(
        "INSERT INTO connection_data_entity (id, tenant_id, connection_parameters, refresh_token_hash) \
         VALUES ($1, $2, $3, NULL)",
    )
    .bind(id)
    .bind(tenant)
    .bind(params)
    .execute(pool)
    .await
    .expect("insert base connection");
}

async fn read_back(pool: &PgPool, id: &str) -> (Value, Option<String>) {
    let row = sqlx::query(
        "SELECT connection_parameters, refresh_token_hash FROM connection_data_entity WHERE id = $1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .expect("read back row");
    let params: Value = row.get("connection_parameters");
    let hash: Option<String> = row.get("refresh_token_hash");
    (params, hash)
}

fn repo(pool: PgPool) -> ConnectionRepository {
    // NoOpCipher → params are stored as plaintext JSON, so assertions can read them back.
    ConnectionRepository::new(pool, Arc::new(NoOpCipher))
}

#[tokio::test]
async fn rotation_persist_bootstraps_on_null_then_guards_on_generation() {
    let fixture = PgFixture::start().await;
    let repo = repo(fixture.pool.clone());
    let tenant = "t_rot";
    let id = "conn_rot";

    insert_conn(
        &fixture.pool,
        id,
        tenant,
        &json!({ "refresh_token": "T0", "access_token": "A0" }),
    )
    .await;

    // 1) Bootstrap: stored hash is NULL → the write lands regardless of expected.
    let rows = repo
        .persist_refreshed_oauth(
            id,
            tenant,
            &json!({ "refresh_token": "T1", "access_token": "A1" }),
            Some("g0"),
            Some("g1"),
        )
        .await
        .expect("bootstrap persist");
    assert_eq!(rows, 1, "NULL hash must bootstrap");
    let (params, hash) = read_back(&fixture.pool, id).await;
    assert_eq!(params["refresh_token"], "T1");
    assert_eq!(params["access_token"], "A1");
    assert_eq!(hash.as_deref(), Some("g1"));

    // 2) Matching generation: expected == stored → the write lands.
    let rows = repo
        .persist_refreshed_oauth(
            id,
            tenant,
            &json!({ "refresh_token": "T2", "access_token": "A2" }),
            Some("g1"),
            Some("g2"),
        )
        .await
        .expect("matching persist");
    assert_eq!(rows, 1, "matching hash must update");
    let (params, hash) = read_back(&fixture.pool, id).await;
    assert_eq!(params["refresh_token"], "T2");
    assert_eq!(hash.as_deref(), Some("g2"));

    // 3) Stale generation (concurrent rotation already advanced the row): expected
    //    ("g1") no longer matches stored ("g2") → the guard rejects the write.
    let rows = repo
        .persist_refreshed_oauth(
            id,
            tenant,
            &json!({ "refresh_token": "T3", "access_token": "A3" }),
            Some("g1"),
            Some("g3"),
        )
        .await
        .expect("stale persist call succeeds at the SQL level");
    assert_eq!(rows, 0, "stale hash must lose the optimistic guard");
    let (params, hash) = read_back(&fixture.pool, id).await;
    // Row is unchanged — the winner's token is preserved, not clobbered.
    assert_eq!(params["refresh_token"], "T2");
    assert_eq!(hash.as_deref(), Some("g2"));
}

#[tokio::test]
async fn a_reauthorization_starts_a_new_rotation_chain() {
    let fixture = PgFixture::start().await;
    let repo = repo(fixture.pool.clone());
    let tenant = "t_reauth";
    let id = "conn_reauth";

    insert_conn(&fixture.pool, id, tenant, &json!({ "refresh_token": "T0" })).await;
    let rows = repo
        .persist_refreshed_oauth(
            id,
            tenant,
            &json!({ "refresh_token": "T1" }),
            Some("g0"),
            Some("g1"),
        )
        .await
        .expect("first chain rotates");
    assert_eq!(rows, 1);

    // The user authorizes again: a new refresh token, from a new chain,
    // sealed as an authorization leaves it. Its guard is its envelope's.
    let reauthorized = json!({ "$sealed": { "envelope": { "ct": "R0" }, "policy": "p" } });
    repo.update_parameters_and_status(
        id,
        tenant,
        &json!({ "refresh_token": reauthorized }),
        "ACTIVE",
    )
    .await
    .expect("re-authorization is stored");
    let r0 = refresh_token_guard(&reauthorized).expect("a sealed token has a guard");
    let (_, hash) = read_back(&fixture.pool, id).await;
    assert_eq!(
        hash.as_deref(),
        Some(r0.as_str()),
        "the guard follows the new chain, not the old one"
    );
    // A token that is not sealed has none.
    assert_eq!(refresh_token_guard(&json!("R0")), None);

    // A refresh that started on the old chain finishes late: it must not
    // overwrite the new authorization's tokens.
    let rows = repo
        .persist_refreshed_oauth(
            id,
            tenant,
            &json!({ "refresh_token": "T2" }),
            Some("g1"),
            Some("g2"),
        )
        .await
        .expect("stale persist call succeeds at the SQL level");
    assert_eq!(rows, 0, "the old chain must lose to the new authorization");
    let (params, _) = read_back(&fixture.pool, id).await;
    assert_eq!(params["refresh_token"], reauthorized);

    // The new chain's first rotation is persisted.
    let rows = repo
        .persist_refreshed_oauth(
            id,
            tenant,
            &json!({ "refresh_token": "R1" }),
            Some(r0.as_str()),
            Some("r1"),
        )
        .await
        .expect("new chain rotates");
    assert_eq!(rows, 1, "a rotation after reconnecting must be persisted");
    let (params, hash) = read_back(&fixture.pool, id).await;
    assert_eq!(params["refresh_token"], "R1");
    assert_eq!(hash.as_deref(), Some("r1"));
}
