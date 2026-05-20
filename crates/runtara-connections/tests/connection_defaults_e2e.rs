use std::collections::HashMap;
use std::sync::Arc;

use runtara_connections::crypto::noop::NoOpCipher;
use runtara_connections::repository::connections::ConnectionRepository;
use runtara_connections::service::connections::ConnectionService;
use runtara_connections::{ConnectionStatus, CreateConnectionRequest, IntegrationCompatibility};
use serde_json::json;
use sqlx::PgPool;
use testcontainers::ContainerAsync;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;

struct PgFixture {
    pool: PgPool,
    _container: ContainerAsync<Postgres>,
}

impl PgFixture {
    async fn start() -> Option<Self> {
        let container = Postgres::default().start().await.ok()?;
        let host = container.get_host().await.ok()?;
        let port = container.get_host_port_ipv4(5432).await.ok()?;
        let url = format!("postgres://postgres:postgres@{host}:{port}/postgres");
        let pool = PgPool::connect(&url).await.ok()?;
        create_schema(&pool).await.ok()?;
        Some(Self {
            pool,
            _container: container,
        })
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
        CREATE UNIQUE INDEX idx_connections_default_file_storage
            ON connection_data_entity (tenant_id)
            WHERE is_default_file_storage = TRUE
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

    sqlx::query(
        r#"
        CREATE INDEX idx_connection_defaults_connection_id
            ON connection_defaults (connection_id)
        "#,
    )
    .execute(pool)
    .await?;

    Ok(())
}

fn test_compatibility() -> Arc<IntegrationCompatibility> {
    // Covers the agents exercised by these tests; the `object_storage`
    // platform bucket is installed automatically by
    // `IntegrationCompatibility::new`.
    let mut by_default_for: HashMap<String, Vec<String>> = HashMap::new();
    by_default_for.insert("object_model".to_string(), vec!["postgres".to_string()]);
    Arc::new(IntegrationCompatibility::new(by_default_for))
}

fn service(pool: PgPool) -> (Arc<ConnectionRepository>, ConnectionService) {
    let repo = Arc::new(ConnectionRepository::new(pool, Arc::new(NoOpCipher)));
    let service = ConnectionService::new(repo.clone(), test_compatibility());
    (repo, service)
}

fn create_request(
    title: &str,
    integration_id: &str,
    default_for: Option<Vec<&str>>,
) -> CreateConnectionRequest {
    CreateConnectionRequest {
        title: title.to_string(),
        connection_subtype: None,
        connection_parameters: Some(json!({
            "database_url": format!("postgres://example/{title}")
        })),
        integration_id: Some(integration_id.to_string()),
        rate_limit_config: None,
        valid_until: None,
        status: Some(ConnectionStatus::Active),
        is_default_file_storage: None,
        default_for: default_for.map(|values| values.into_iter().map(str::to_string).collect()),
    }
}

#[tokio::test]
async fn default_connection_moves_between_compatible_object_model_connections() {
    let Some(fixture) = PgFixture::start().await else {
        eprintln!("Skipping connection defaults e2e: Docker/Postgres unavailable");
        return;
    };
    let tenant_id = "tenant_defaults_e2e";
    let (repo, service) = service(fixture.pool.clone());

    let first_id = service
        .create_connection(
            create_request(
                "Object Model Postgres A",
                "postgres",
                Some(vec!["object_model"]),
            ),
            tenant_id,
        )
        .await
        .expect("create first default postgres connection");
    let second_id = service
        .create_connection(
            create_request(
                "Object Model Postgres B",
                "postgres",
                Some(vec!["object_model"]),
            ),
            tenant_id,
        )
        .await
        .expect("create replacement default postgres connection");

    let first = service
        .get_connection(&first_id, tenant_id)
        .await
        .expect("load first connection");
    let second = service
        .get_connection(&second_id, tenant_id)
        .await
        .expect("load second connection");

    assert!(first.default_for.is_empty());
    assert_eq!(second.default_for, vec!["object_model".to_string()]);

    let default = repo
        .get_default_connection_with_parameters(tenant_id, "object_model")
        .await
        .expect("load default object_model connection")
        .expect("default exists");
    assert_eq!(default.id, second_id);
    assert_eq!(default.integration_id.as_deref(), Some("postgres"));
    assert_eq!(
        default
            .connection_parameters
            .as_ref()
            .and_then(|value| value.get("database_url"))
            .and_then(|value| value.as_str()),
        Some("postgres://example/Object Model Postgres B")
    );
}

#[tokio::test]
async fn object_storage_default_bridges_legacy_file_storage_flag() {
    let Some(fixture) = PgFixture::start().await else {
        eprintln!("Skipping connection defaults e2e: Docker/Postgres unavailable");
        return;
    };
    let tenant_id = "tenant_file_storage_e2e";
    let (_repo, service) = service(fixture.pool.clone());

    let mut s3_request = create_request("S3 Storage", "s3_compatible", None);
    s3_request.is_default_file_storage = Some(true);
    let s3_id = service
        .create_connection(s3_request, tenant_id)
        .await
        .expect("create s3 default");

    let azure_id = service
        .create_connection(
            create_request(
                "Azure Blob Storage",
                "azure_blob_storage",
                Some(vec!["object_storage"]),
            ),
            tenant_id,
        )
        .await
        .expect("create replacement object storage default");

    let s3 = service
        .get_connection(&s3_id, tenant_id)
        .await
        .expect("load old default");
    let azure = service
        .get_connection(&azure_id, tenant_id)
        .await
        .expect("load new default");

    assert!(!s3.is_default_file_storage);
    assert!(s3.default_for.is_empty());
    assert!(azure.is_default_file_storage);
    assert_eq!(azure.default_for, vec!["object_storage".to_string()]);
}
