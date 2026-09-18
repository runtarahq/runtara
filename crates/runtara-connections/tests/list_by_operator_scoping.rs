//! `list_by_operator` is what backs the step editor's connection picker
//! (`GET /api/runtime/connections/operator/{agent}`). The integration-id list
//! is the ONLY thing scoping its result to the caller's agent, so these tests
//! pin both halves of that contract: the filter selects, and an empty filter
//! never degrades into "everything in the tenant".

use std::sync::Arc;

use runtara_connections::crypto::noop::NoOpCipher;
use runtara_connections::repository::connections::ConnectionRepository;
use sqlx::PgPool;
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
        create_schema(&pool)
            .await
            .expect("required connection test schema");
        Self {
            pool,
            _container: container,
        }
    }
}

async fn create_schema(pool: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        CREATE TABLE connection_data_entity (
            id VARCHAR(255) PRIMARY KEY,
            tenant_id VARCHAR(255) NOT NULL,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            valid_until TIMESTAMPTZ DEFAULT NULL,
            updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            title VARCHAR(255) NOT NULL UNIQUE,
            connection_subtype VARCHAR(255) DEFAULT NULL,
            connection_parameters JSONB DEFAULT NULL,
            integration_id VARCHAR(255) DEFAULT NULL,
            status VARCHAR(50) NOT NULL DEFAULT 'UNKNOWN',
            rate_limit_config JSONB DEFAULT NULL,
            is_default_file_storage BOOLEAN NOT NULL DEFAULT FALSE
        )
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE connection_defaults (
            tenant_id VARCHAR(255) NOT NULL,
            default_for VARCHAR(255) NOT NULL,
            connection_id VARCHAR(255) NOT NULL,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            PRIMARY KEY (tenant_id, default_for),
            CONSTRAINT fk_connection_defaults_connection
                FOREIGN KEY (connection_id)
                REFERENCES connection_data_entity(id)
                ON DELETE CASCADE
        )
        "#,
    )
    .execute(pool)
    .await?;

    Ok(())
}

const TENANT: &str = "tenant_list_by_operator";

/// Seed one connection per (id, integration, status). Written straight to the
/// table rather than through the service: this is a query-scoping test, and
/// per-integration valid parameter shapes are irrelevant to it.
async fn seed(pool: &PgPool) {
    let rows = [
        ("api-key-active", TENANT, "http_api_key", "ACTIVE"),
        ("bearer-active", TENANT, "http_bearer", "ACTIVE"),
        (
            "api-key-broken",
            TENANT,
            "http_api_key",
            "INVALID_CREDENTIALS",
        ),
        ("mcp-active", TENANT, "mcp", "ACTIVE"),
        ("postgres-active", TENANT, "postgres", "ACTIVE"),
        ("other-tenant-key", "someone_else", "http_api_key", "ACTIVE"),
    ];

    for (id, tenant, integration_id, status) in rows {
        sqlx::query(
            r#"
            INSERT INTO connection_data_entity (id, tenant_id, title, integration_id, status)
            VALUES ($1, $2, $3, $4, $5)
            "#,
        )
        .bind(id)
        .bind(tenant)
        .bind(format!("{id} ({tenant})"))
        .bind(integration_id)
        .bind(status)
        .execute(pool)
        .await
        .expect("seed connection row");
    }
}

fn repository(pool: PgPool) -> ConnectionRepository {
    ConnectionRepository::new(pool, Arc::new(NoOpCipher))
}

fn ids(connections: &[runtara_connections::ConnectionDto]) -> Vec<String> {
    let mut ids: Vec<String> = connections.iter().map(|c| c.id.clone()).collect();
    ids.sort();
    ids
}

#[tokio::test]
async fn filters_to_the_requested_integrations_within_the_tenant() {
    let fixture = PgFixture::start().await;
    seed(&fixture.pool).await;
    let repo = repository(fixture.pool.clone());

    let found = repo
        .list_by_operator(
            TENANT,
            &["http_api_key".to_string(), "http_bearer".to_string()],
            None,
        )
        .await
        .expect("list by operator");

    assert_eq!(
        ids(&found),
        vec!["api-key-active", "api-key-broken", "bearer-active"],
        "only the requested integrations, and only this tenant's rows"
    );
}

#[tokio::test]
async fn status_narrows_within_the_integration_filter() {
    let fixture = PgFixture::start().await;
    seed(&fixture.pool).await;
    let repo = repository(fixture.pool.clone());

    let found = repo
        .list_by_operator(TENANT, &["http_api_key".to_string()], Some("ACTIVE"))
        .await
        .expect("list by operator with status");

    assert_eq!(ids(&found), vec!["api-key-active"]);
}

/// Regression: an agent with no integration ids must resolve to no
/// connections. This used to hold only when `status` was absent — with a
/// status filter the integration predicate was dropped entirely and every
/// connection in the tenant came back, whatever agent was asked for.
#[tokio::test]
async fn an_empty_integration_filter_never_returns_the_whole_tenant() {
    let fixture = PgFixture::start().await;
    seed(&fixture.pool).await;
    let repo = repository(fixture.pool.clone());

    for status in [None, Some("ACTIVE")] {
        let found = repo
            .list_by_operator(TENANT, &[], status)
            .await
            .expect("list by operator with an empty integration filter");

        assert!(
            found.is_empty(),
            "empty integration filter (status={status:?}) must fail closed, got: {:?}",
            ids(&found)
        );
    }
}

#[tokio::test]
async fn unmatched_integration_ids_return_nothing() {
    let fixture = PgFixture::start().await;
    seed(&fixture.pool).await;
    let repo = repository(fixture.pool.clone());

    let found = repo
        .list_by_operator(TENANT, &["stripe_api_key".to_string()], None)
        .await
        .expect("list by operator");

    assert!(found.is_empty(), "got: {:?}", ids(&found));
}

/// Decryption is deliberately counted independently of the selected output:
/// returning None after decrypting would still violate the trusted boundary.
#[tokio::test]
async fn trusted_resolution_checks_tenant_and_exact_type_before_decryption() {
    use runtara_connections::crypto::{CipherError, CredentialCipher};
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct CountingCipher(AtomicUsize);
    impl CredentialCipher for CountingCipher {
        fn encrypt(&self, value: &serde_json::Value) -> Result<serde_json::Value, CipherError> {
            Ok(value.clone())
        }
        fn decrypt(&self, value: &serde_json::Value) -> Result<serde_json::Value, CipherError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(value.clone())
        }
        fn key_id(&self) -> &str {
            "test"
        }
        fn is_encrypting(&self) -> bool {
            true
        }
    }
    let fixture = PgFixture::start().await;
    sqlx::query("INSERT INTO connection_data_entity (id, tenant_id, title, integration_id, status, connection_parameters) VALUES ($1, $2, $1, $3, 'ACTIVE', '{}'::jsonb)")
        .bind("trusted-s3").bind(TENANT).bind("s3_compatible")
        .execute(&fixture.pool).await.unwrap();
    let cipher = Arc::new(CountingCipher(AtomicUsize::new(0)));
    let repo = ConnectionRepository::new(fixture.pool.clone(), cipher.clone());
    for (tenant, types) in [
        ("other-tenant", vec!["s3_compatible".to_owned()]),
        (TENANT, vec!["azure_blob_storage".to_owned()]),
        (TENANT, vec!["aws_credentials".to_owned()]),
        (TENANT, vec!["object_storage".to_owned()]),
        (TENANT, vec!["*".to_owned()]),
        (TENANT, vec![]),
    ] {
        assert!(
            repo.get_with_parameters_for_types("trusted-s3", tenant, &types)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(cipher.0.load(Ordering::SeqCst), 0);
    }
    assert!(
        repo.get_with_parameters_for_types("trusted-s3", TENANT, &["s3_compatible".to_owned()])
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(cipher.0.load(Ordering::SeqCst), 1);
}
