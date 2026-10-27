//! Per-tenant credential keys against a real Postgres: rotating one tenant's
//! key re-encrypts only that tenant's rows, and every other tenant's stored
//! ciphertext stays byte for byte as it was (M7.4).

use std::sync::Arc;

use runtara_connections::crypto::aes_gcm::AesGcmCipher;
use runtara_connections::repository::connections::ConnectionRepository;
use runtara_connections::types::{ConnectionStatus, CreateConnectionRequest};
use runtara_connections::{CredentialCipher, TenantKeyedCipher};
use serde_json::{Value, json};
use sqlx::PgPool;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;
use zeroize::Zeroizing;

fn keyring(current: (u8, &str), retired: &[(u8, &str)]) -> Arc<dyn CredentialCipher> {
    let mut cipher = AesGcmCipher::new(Zeroizing::new(vec![current.0; 32]), current.1).unwrap();
    for (byte, kid) in retired {
        cipher = cipher
            .with_retired_key(Zeroizing::new(vec![*byte; 32]), *kid)
            .unwrap();
    }
    Arc::new(cipher)
}

async fn stored(pool: &PgPool, id: &str) -> Value {
    sqlx::query_scalar("SELECT connection_parameters FROM connection_data_entity WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn rotating_one_tenants_key_leaves_other_tenants_rows_untouched() {
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

    let beta = keyring((3, "beta-1"), &[]);
    let before = Arc::new(
        TenantKeyedCipher::new(keyring((1, "default-1"), &[]))
            .with_tenant("acme", keyring((2, "acme-1"), &[]))
            .with_tenant("beta", beta.clone()),
    );
    let repo = ConnectionRepository::new(pool.clone(), before);
    for (tenant, id) in [("acme", "acme-conn"), ("beta", "beta-conn")] {
        let request: CreateConnectionRequest = serde_json::from_value(json!({
            "title": id,
            "integrationId": "http_bearer",
            "connectionParameters": {"token": format!("{tenant}-secret")}
        }))
        .unwrap();
        repo.create(&request, tenant, id, &ConnectionStatus::Active, &[])
            .await
            .unwrap();
    }
    assert_eq!(stored(&pool, "acme-conn").await["kid"], "acme-1");
    let beta_before = stored(&pool, "beta-conn").await;
    assert_eq!(beta_before["kid"], "beta-1");

    // acme rotates: its new key is current, the old one retired, and only
    // acme's rows are re-encrypted.
    let rotating = Arc::new(
        TenantKeyedCipher::new(keyring((1, "default-1"), &[]))
            .with_tenant("acme", keyring((4, "acme-2"), &[(2, "acme-1")]))
            .with_tenant("beta", beta.clone()),
    );
    let stats = ConnectionRepository::new(pool.clone(), rotating)
        .reencrypt_all(Some("acme"))
        .await
        .unwrap();
    assert_eq!((stats.reencrypted, stats.failed), (1, 0));
    assert_eq!(stored(&pool, "acme-conn").await["kid"], "acme-2");
    assert_eq!(
        stored(&pool, "beta-conn").await,
        beta_before,
        "another tenant's row is not rewritten"
    );

    // With acme's old key gone, both tenants still read their credentials.
    let rotated = ConnectionRepository::new(
        pool.clone(),
        Arc::new(
            TenantKeyedCipher::new(keyring((1, "default-1"), &[]))
                .with_tenant("acme", keyring((4, "acme-2"), &[]))
                .with_tenant("beta", beta),
        ),
    );
    for (tenant, id) in [("acme", "acme-conn"), ("beta", "beta-conn")] {
        let params = rotated
            .get_with_parameters(id, tenant)
            .await
            .unwrap()
            .unwrap()
            .connection_parameters
            .unwrap();
        assert_eq!(params["token"], format!("{tenant}-secret"));
    }
}
