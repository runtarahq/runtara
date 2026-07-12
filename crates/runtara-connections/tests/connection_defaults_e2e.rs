use std::collections::HashMap;
use std::sync::Arc;

use runtara_connections::crypto::noop::NoOpCipher;
use runtara_connections::repository::connections::ConnectionRepository;
use runtara_connections::service::connections::ConnectionService;
use runtara_connections::{CreateConnectionRequest, IntegrationCompatibility, RateLimitConfigDto};
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
    let connection_parameters = match integration_id {
        "postgres" => json!({
            "database_url": format!("postgres://example/{title}")
        }),
        "stripe_api_key" => json!({ "secret_key": "sk_test_fixture" }),
        "http_bearer" => json!({
            "token": "fixture-token",
            "base_url": "https://api.example.com"
        }),
        "s3_compatible" => json!({
            "endpoint": "https://s3.example.com",
            "access_key_id": "fixture-access-key",
            "secret_access_key": "fixture-secret",
            "region": "us-east-1"
        }),
        "azure_blob_storage" => json!({
            "account_name": "fixtureaccount",
            "account_key": "Zml4dHVyZS1rZXk="
        }),
        other => panic!("missing valid connection fixture for {other}"),
    };
    CreateConnectionRequest {
        title: title.to_string(),
        connection_subtype: None,
        connection_parameters: Some(connection_parameters),
        integration_id: Some(integration_id.to_string()),
        rate_limit_config: None,
        valid_until: None,
        is_default_file_storage: None,
        default_for: default_for.map(|values| values.into_iter().map(str::to_string).collect()),
    }
}

#[tokio::test]
async fn default_connection_moves_between_compatible_object_model_connections() {
    let fixture = PgFixture::start().await;
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
async fn create_applies_default_rate_limit_for_known_integration() {
    let fixture = PgFixture::start().await;
    let tenant_id = "tenant_rate_limit_default";
    let (_repo, service) = service(fixture.pool.clone());

    // No rate_limit_config supplied → the create path should snapshot the
    // per-integration default (SYN-493).
    let id = service
        .create_connection(
            create_request("Stripe Live", "stripe_api_key", None),
            tenant_id,
        )
        .await
        .expect("create stripe connection");

    let conn = service
        .get_connection(&id, tenant_id)
        .await
        .expect("load stripe connection");
    let cfg = conn
        .rate_limit_config
        .expect("a known integration must receive its default rate limit at create time");
    assert_eq!(cfg.requests_per_second, 20);
    assert_eq!(cfg.burst_size, 40);
    assert!(cfg.retry_on_limit);
}

#[tokio::test]
async fn create_preserves_explicit_rate_limit_over_default() {
    let fixture = PgFixture::start().await;
    let tenant_id = "tenant_rate_limit_explicit";
    let (_repo, service) = service(fixture.pool.clone());

    let mut request = create_request("Stripe Custom", "stripe_api_key", None);
    request.rate_limit_config = Some(RateLimitConfigDto {
        requests_per_second: 3,
        burst_size: 3,
        retry_on_limit: false,
        max_retries: 0,
        max_wait_ms: 1000,
    });
    let id = service
        .create_connection(request, tenant_id)
        .await
        .expect("create stripe connection with explicit limit");

    let conn = service
        .get_connection(&id, tenant_id)
        .await
        .expect("load stripe connection");
    let cfg = conn
        .rate_limit_config
        .expect("explicit rate limit must be preserved");
    // The default (20/40) must NOT clobber the user-supplied config.
    assert_eq!(cfg.requests_per_second, 3);
    assert_eq!(cfg.burst_size, 3);
    assert!(!cfg.retry_on_limit);
}

#[tokio::test]
async fn create_leaves_opt_out_integration_without_rate_limit() {
    let fixture = PgFixture::start().await;
    let tenant_id = "tenant_rate_limit_opt_out";
    let (_repo, service) = service(fixture.pool.clone());

    // http_bearer is on RATE_LIMIT_OPT_OUT (generic target) → no default applied.
    // Unlike the derived-base-URL types the other tests use, http_bearer's
    // schema requires a token and an https base_url at creation time.
    let mut request = create_request("Generic Bearer API", "http_bearer", None);
    request.connection_parameters = Some(json!({
        "token": "test-token",
        "base_url": "https://api.example.com",
    }));
    let id = service
        .create_connection(request, tenant_id)
        .await
        .expect("create generic bearer connection");

    let conn = service
        .get_connection(&id, tenant_id)
        .await
        .expect("load generic bearer connection");
    assert!(
        conn.rate_limit_config.is_none(),
        "opt-out integration types must not receive a default rate limit",
    );
}

#[tokio::test]
async fn create_rejects_a_zero_rate_limit() {
    let fixture = PgFixture::start().await;
    let tenant_id = "tenant_rate_limit_zero";
    let (_repo, service) = service(fixture.pool.clone());

    // requests_per_second=0 looks "configured" but the limiter treats it as
    // "no limit" — a silent bypass. The service must reject it (SYN-500).
    let mut request = create_request("Zero RPS Stripe", "stripe_api_key", None);
    request.rate_limit_config = Some(RateLimitConfigDto {
        requests_per_second: 0,
        burst_size: 4,
        retry_on_limit: true,
        max_retries: 3,
        max_wait_ms: 60000,
    });
    let result = service.create_connection(request, tenant_id).await;
    assert!(
        result.is_err(),
        "creating a connection with requests_per_second=0 must be rejected"
    );
}

#[tokio::test]
async fn object_storage_default_bridges_legacy_file_storage_flag() {
    let fixture = PgFixture::start().await;
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

#[tokio::test]
async fn stale_update_cannot_move_default_assignments_or_file_storage_flag() {
    let fixture = PgFixture::start().await;
    let tenant_id = "tenant_stale_default_e2e";
    let (repo, service) = service(fixture.pool.clone());

    let first_id = service
        .create_connection(
            create_request(
                "Default storage",
                "s3_compatible",
                Some(vec!["object_storage"]),
            ),
            tenant_id,
        )
        .await
        .expect("create initial default");
    let second_id = service
        .create_connection(
            create_request("Candidate storage", "azure_blob_storage", None),
            tenant_id,
        )
        .await
        .expect("create candidate");
    let opened_version = service
        .get_connection(&second_id, tenant_id)
        .await
        .unwrap()
        .updated_at;
    let rename = serde_json::from_value(json!({
        "version": opened_version,
        "title": "Candidate changed elsewhere"
    }))
    .unwrap();
    service
        .update_connection(&second_id, tenant_id, rename)
        .await
        .expect("concurrent rename");

    let stale = serde_json::from_value(json!({
        "version": opened_version,
        "isDefaultFileStorage": true,
        "defaultFor": ["object_storage"]
    }))
    .unwrap();
    assert!(matches!(
        service
            .update_connection(&second_id, tenant_id, stale)
            .await,
        Err(runtara_connections::service::connections::ServiceError::Conflict(_))
    ));
    assert!(
        service
            .get_connection(&first_id, tenant_id)
            .await
            .unwrap()
            .is_default_file_storage
    );
    assert!(
        !service
            .get_connection(&second_id, tenant_id)
            .await
            .unwrap()
            .is_default_file_storage
    );
    assert_eq!(
        repo.get_default_connection_id(tenant_id, "object_storage")
            .await
            .unwrap()
            .as_deref(),
        Some(first_id.as_str())
    );
}

#[tokio::test]
async fn failed_default_create_rolls_back_singleton_and_mapping_changes() {
    let fixture = PgFixture::start().await;
    let tenant_id = "tenant_failed_default_create_e2e";
    let (repo, service) = service(fixture.pool.clone());

    let existing_id = service
        .create_connection(
            create_request(
                "Existing default storage",
                "s3_compatible",
                Some(vec!["object_storage"]),
            ),
            tenant_id,
        )
        .await
        .expect("create existing default");

    let duplicate = create_request(
        "Existing default storage",
        "azure_blob_storage",
        Some(vec!["object_storage"]),
    );
    assert!(
        service
            .create_connection(duplicate, tenant_id)
            .await
            .is_err()
    );

    let existing = service
        .get_connection(&existing_id, tenant_id)
        .await
        .expect("existing default survives failed create");
    assert!(existing.is_default_file_storage);
    assert_eq!(existing.default_for, vec!["object_storage".to_string()]);
    assert_eq!(
        repo.get_default_connection_id(tenant_id, "object_storage")
            .await
            .unwrap()
            .as_deref(),
        Some(existing_id.as_str())
    );
}
