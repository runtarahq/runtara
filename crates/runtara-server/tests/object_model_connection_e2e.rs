use std::collections::HashMap;
use std::sync::Arc;

use runtara_connections::crypto::noop::NoOpCipher;
use runtara_connections::repository::connections::ConnectionRepository;
use runtara_connections::service::connections::ConnectionService;
use runtara_connections::{
    ConnectionStatus, ConnectionsConfig, ConnectionsFacade, ConnectionsState,
    CreateConnectionRequest, IntegrationCompatibility,
};
use runtara_server::api::dto::object_model::{
    ColumnDefinition, ColumnType, CreateSchemaRequest, TextIndexKind,
};
use runtara_server::api::repositories::object_model::ObjectStoreManager;
use runtara_server::api::services::object_model::SchemaService;
use serde_json::json;
use sqlx::PgPool;
use testcontainers::core::{ContainerPort, IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};

const POSTGRES_PORT: u16 = 5432;

struct PgVectorFixture {
    server_pool: PgPool,
    default_object_url: String,
    alternate_object_url: String,
    _container: ContainerAsync<GenericImage>,
}

impl PgVectorFixture {
    async fn start() -> Option<Self> {
        let container = GenericImage::new("pgvector/pgvector", "pg16")
            .with_exposed_port(ContainerPort::Tcp(POSTGRES_PORT))
            .with_wait_for(WaitFor::message_on_stdout(
                "database system is ready to accept connections",
            ))
            .with_env_var("POSTGRES_USER", "postgres")
            .with_env_var("POSTGRES_PASSWORD", "postgres")
            .with_env_var("POSTGRES_DB", "postgres")
            .start()
            .await
            .ok()?;

        let host = container.get_host().await.ok()?;
        let port = container
            .get_host_port_ipv4(POSTGRES_PORT.tcp())
            .await
            .ok()?;
        let root_url = format!("postgres://postgres:postgres@{host}:{port}/postgres");
        let server_pool = PgPool::connect(&root_url).await.ok()?;

        sqlx::query(r#"CREATE DATABASE runtara_object_default"#)
            .execute(&server_pool)
            .await
            .ok()?;
        sqlx::query(r#"CREATE DATABASE runtara_object_alternate"#)
            .execute(&server_pool)
            .await
            .ok()?;
        create_connection_schema(&server_pool).await.ok()?;

        Some(Self {
            server_pool,
            default_object_url: format!(
                "postgres://postgres:postgres@{host}:{port}/runtara_object_default"
            ),
            alternate_object_url: format!(
                "postgres://postgres:postgres@{host}:{port}/runtara_object_alternate"
            ),
            _container: container,
        })
    }
}

async fn create_connection_schema(pool: &PgPool) -> Result<(), sqlx::Error> {
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

fn test_compatibility() -> Arc<IntegrationCompatibility> {
    let mut by_default_for: HashMap<String, Vec<String>> = HashMap::new();
    by_default_for.insert("object_model".to_string(), vec!["postgres".to_string()]);
    Arc::new(IntegrationCompatibility::new(by_default_for))
}

fn test_agent_catalog() -> Arc<runtara_dsl::agent_meta::AgentCatalog> {
    Arc::new(runtara_dsl::agent_meta::AgentCatalog::from_agents(
        Vec::new(),
    ))
}

fn facade(pool: PgPool) -> ConnectionsFacade {
    ConnectionsFacade::new(ConnectionsState::from_config(ConnectionsConfig {
        db_pool: pool,
        redis_manager: None,
        public_base_url: "http://localhost".to_string(),
        http_client: reqwest::Client::new(),
        cipher: Arc::new(NoOpCipher),
        compatibility: test_compatibility(),
        agent_catalog: test_agent_catalog(),
        connection_events: None,
    }))
}

fn schema_request(name: &str) -> CreateSchemaRequest {
    CreateSchemaRequest {
        name: name.to_string(),
        description: None,
        table_name: name.to_string(),
        columns: vec![ColumnDefinition {
            name: "name".to_string(),
            column_type: ColumnType::String,
            nullable: false,
            unique: false,
            default_value: None,
            text_index: TextIndexKind::None,
        }],
        indexes: None,
    }
}

#[tokio::test]
async fn object_model_routes_schemas_to_selected_connection_database() {
    let Some(fixture) = PgVectorFixture::start().await else {
        eprintln!("Skipping Object Model connection e2e: Docker/pgvector Postgres unavailable");
        return;
    };

    let tenant_id = "tenant_object_model_e2e";
    let facade = Arc::new(facade(fixture.server_pool.clone()));
    facade
        .ensure_default_connection(
            tenant_id,
            "object_model",
            "Default Object Model DB".to_string(),
            "postgres".to_string(),
            json!({ "database_url": fixture.default_object_url }),
        )
        .await
        .expect("seed default object_model connection");

    let repo = Arc::new(ConnectionRepository::new(
        fixture.server_pool.clone(),
        Arc::new(NoOpCipher),
    ));
    let connection_service = ConnectionService::new(repo, test_compatibility());
    let alternate_connection_id = connection_service
        .create_connection(
            CreateConnectionRequest {
                title: "Alternate Object Model DB".to_string(),
                connection_subtype: None,
                connection_parameters: Some(json!({
                    "database_url": fixture.alternate_object_url
                })),
                integration_id: Some("postgres".to_string()),
                rate_limit_config: None,
                valid_until: None,
                status: Some(ConnectionStatus::Active),
                is_default_file_storage: None,
                default_for: None,
            },
            tenant_id,
        )
        .await
        .expect("create alternate object_model connection");

    let manager = Arc::new(ObjectStoreManager::new(fixture.default_object_url));
    let schemas = SchemaService::new(manager, facade);

    schemas
        .create_schema(schema_request("default_orders"), tenant_id, None)
        .await
        .expect("create schema in default database");
    schemas
        .create_schema(
            schema_request("alternate_orders"),
            tenant_id,
            Some(&alternate_connection_id),
        )
        .await
        .expect("create schema in selected database");

    let (default_schemas, _) = schemas
        .list_schemas(tenant_id, 0, 100, None)
        .await
        .expect("list default database schemas");
    let (alternate_schemas, _) = schemas
        .list_schemas(tenant_id, 0, 100, Some(&alternate_connection_id))
        .await
        .expect("list selected database schemas");

    let default_names: Vec<_> = default_schemas
        .iter()
        .map(|schema| schema.name.as_str())
        .collect();
    let alternate_names: Vec<_> = alternate_schemas
        .iter()
        .map(|schema| schema.name.as_str())
        .collect();

    assert!(default_names.contains(&"default_orders"));
    assert!(!default_names.contains(&"alternate_orders"));
    assert!(alternate_names.contains(&"alternate_orders"));
    assert!(!alternate_names.contains(&"default_orders"));
}
