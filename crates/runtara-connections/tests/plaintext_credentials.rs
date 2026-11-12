//! Credentials stored as plaintext (by releases before encryption was
//! mandatory, or written straight into the table) against a real Postgres:
//! they are never served as they are, `encrypt_plaintext` seals exactly those
//! rows, and the sealed rows read back.

use std::sync::Arc;

use runtara_connections::CredentialCipher;
use runtara_connections::crypto::aes_gcm::AesGcmCipher;
use runtara_connections::repository::connections::ConnectionRepository;
use runtara_connections::types::{ConnectionStatus, CreateConnectionRequest};
use serde_json::{Value, json};
use sqlx::PgPool;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;
use zeroize::Zeroizing;

fn cipher() -> Arc<dyn CredentialCipher> {
    Arc::new(AesGcmCipher::new(Zeroizing::new(vec![9u8; 32]), "k1").unwrap())
}

async fn stored(pool: &PgPool, id: &str) -> Value {
    sqlx::query_scalar("SELECT connection_parameters FROM connection_data_entity WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn insert_plaintext(pool: &PgPool, tenant: &str, id: &str, params: Value) {
    sqlx::query(
        "INSERT INTO connection_data_entity (id, tenant_id, integration_id, connection_parameters)
         VALUES ($1, $2, 'http_bearer', $3)",
    )
    .bind(id)
    .bind(tenant)
    .bind(params)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn plaintext_rows_are_refused_until_encrypted_then_read_back() {
    let container = Postgres::default()
        .start()
        .await
        .expect("required Docker Postgres container must start");
    let url = format!(
        "postgres://postgres:postgres@{}:{}/postgres",
        container.get_host().await.unwrap(),
        container.get_host_port_ipv4(5432).await.unwrap()
    );
    let pool = PgPool::connect(&url).await.unwrap();
    sqlx::query(
        "CREATE TABLE connection_data_entity (
            id VARCHAR(255) PRIMARY KEY,
            tenant_id VARCHAR(255) NOT NULL,
            title VARCHAR(255) NOT NULL DEFAULT 'conn',
            integration_id VARCHAR(255),
            connection_subtype VARCHAR(255),
            connection_parameters JSONB,
            credential_format TEXT NOT NULL DEFAULT 'cipher',
            rate_limit_config JSONB,
            status VARCHAR(64) DEFAULT 'UNKNOWN',
            refresh_token_hash TEXT DEFAULT NULL,
            is_default_file_storage BOOLEAN NOT NULL DEFAULT FALSE,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            valid_until TIMESTAMPTZ DEFAULT NULL,
            updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        )",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TABLE connection_defaults (
            tenant_id VARCHAR(255) NOT NULL,
            default_for VARCHAR(255) NOT NULL,
            connection_id VARCHAR(255) NOT NULL,
            PRIMARY KEY (tenant_id, default_for)
        )",
    )
    .execute(&pool)
    .await
    .unwrap();

    let repo = ConnectionRepository::new(pool.clone(), cipher());

    // One connection written through the repository: already sealed.
    let request: CreateConnectionRequest = serde_json::from_value(json!({
        "title": "sealed",
        "integrationId": "http_bearer",
        "connectionParameters": {"token": "sealed-secret"}
    }))
    .unwrap();
    repo.create(&request, "acme", "sealed", &ConnectionStatus::Active, &[])
        .await
        .unwrap();
    let sealed_before = stored(&pool, "sealed").await;

    // Two left as plaintext by an older release, in two tenants, one of them
    // shaped like a partial envelope (it has `v` and `alg` but no `ct`).
    insert_plaintext(&pool, "acme", "legacy", json!({"token": "legacy-secret"})).await;
    insert_plaintext(
        &pool,
        "beta",
        "lookalike",
        json!({"v": 1, "alg": "aes-256-gcm", "token": "beta-secret"}),
    )
    .await;

    // Plaintext is not served as it is.
    assert!(
        repo.get_with_parameters("legacy", "acme").await.is_err(),
        "a plaintext row must not be read"
    );

    let stats = repo.encrypt_plaintext().await.unwrap();
    assert_eq!(
        (stats.scanned, stats.reencrypted, stats.failed),
        (2, 2, 0),
        "only the plaintext rows are loaded and sealed"
    );
    assert_eq!(
        stored(&pool, "sealed").await,
        sealed_before,
        "an envelope is left byte for byte as it was"
    );
    for id in ["legacy", "lookalike"] {
        let value = stored(&pool, id).await;
        assert_eq!(value["kid"], "k1", "{id} is sealed under the current key");
        assert!(
            value.get("token").is_none(),
            "{id} no longer holds plaintext"
        );
    }

    for (tenant, id, secret) in [
        ("acme", "legacy", "legacy-secret"),
        ("beta", "lookalike", "beta-secret"),
        ("acme", "sealed", "sealed-secret"),
    ] {
        let params = repo
            .get_with_parameters(id, tenant)
            .await
            .unwrap()
            .unwrap()
            .connection_parameters
            .unwrap();
        assert_eq!(params["token"], secret);
    }

    // Nothing left to do: a second boot touches nothing.
    let again = repo.encrypt_plaintext().await.unwrap();
    assert_eq!((again.scanned, again.reencrypted), (0, 0));

    // reencrypt_all still converts plaintext too (the manual path).
    insert_plaintext(&pool, "acme", "late", json!({"token": "late-secret"})).await;
    let all = repo.reencrypt_all(Some("acme")).await.unwrap();
    assert_eq!((all.reencrypted, all.failed), (3, 0));
    assert_eq!(stored(&pool, "late").await["kid"], "k1");
}
