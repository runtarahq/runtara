//! Sealed credentials against a real Postgres: a sealable connection's secret
//! is stored as seal's envelope, the row says so, the tenant's key is kept in
//! `seal_wrapped_key`, and connections that are not sealed stay as they were.

use std::collections::HashMap;
use std::sync::Arc;

use runtara_connections::IntegrationCompatibility;
use runtara_connections::crypto::noop::NoOpCipher;
use runtara_connections::repository::connections::ConnectionRepository;
use runtara_connections::sealed::keys::PgKeySource;
use runtara_connections::sealed::{Allowances, SEALED_MARKER, SealedCredentials, is_sealed};
use runtara_connections::service::connections::{ConnectionService, ServiceError};
use runtara_connections::types::{CreateConnectionRequest, UpdateConnectionRequest};
use serde_json::{Value, json};
use sqlx::PgPool;
use testcontainers::ContainerAsync;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;

const TENANT: &str = "acme";
const SECRET: &str = "s3cr3t-CANARY-token";

struct Fixture {
    pool: PgPool,
    _container: ContainerAsync<Postgres>,
}

impl Fixture {
    async fn new() -> Self {
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
        for statement in [
            "CREATE TABLE connection_data_entity (
                id VARCHAR(255) PRIMARY KEY,
                tenant_id VARCHAR(255) NOT NULL,
                title VARCHAR(255),
                connection_subtype VARCHAR(255),
                connection_parameters JSONB,
                credential_format TEXT NOT NULL DEFAULT 'cipher',
                integration_id VARCHAR(255),
                valid_until TIMESTAMPTZ,
                status VARCHAR(64),
                rate_limit_config JSONB,
                is_default_file_storage BOOLEAN DEFAULT FALSE,
                refresh_token_hash TEXT DEFAULT NULL,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )",
            "CREATE TABLE connection_defaults (
                tenant_id VARCHAR(255) NOT NULL,
                default_for VARCHAR(255) NOT NULL,
                connection_id VARCHAR(255) NOT NULL,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                PRIMARY KEY (tenant_id, default_for)
            )",
            // As the server's migration makes it.
            "CREATE TABLE seal_wrapped_key (
                keyset TEXT NOT NULL,
                kid TEXT NOT NULL,
                tenant_id TEXT NOT NULL,
                public_key TEXT NOT NULL,
                created BIGINT NOT NULL,
                wrap_id TEXT NOT NULL,
                wrapped TEXT NOT NULL,
                stored_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                PRIMARY KEY (keyset, kid)
            )",
        ] {
            sqlx::query(statement).execute(&pool).await.unwrap();
        }
        Self {
            pool,
            _container: container,
        }
    }

    /// seal over this database's key table, as one server instance has it.
    fn sealed(&self) -> Arc<SealedCredentials> {
        Arc::new(
            SealedCredentials::in_process(
                Arc::new(PgKeySource::new(self.pool.clone())),
                &[7_u8; 32],
                Allowances::default(),
            )
            .unwrap(),
        )
    }

    fn service(&self, sealed: Option<Arc<SealedCredentials>>) -> ConnectionService {
        let repository = Arc::new(ConnectionRepository::new(
            self.pool.clone(),
            Arc::new(NoOpCipher),
        ));
        ConnectionService::new(
            repository,
            Arc::new(IntegrationCompatibility::new(HashMap::new())),
        )
        .with_sealed(sealed)
    }

    fn repository(&self) -> ConnectionRepository {
        ConnectionRepository::new(self.pool.clone(), Arc::new(NoOpCipher))
    }

    /// The row as stored: its format and its parameters (the cipher here
    /// stores them as they are).
    async fn row(&self, id: &str) -> (String, Value) {
        sqlx::query_as(
            "SELECT credential_format, connection_parameters FROM connection_data_entity WHERE id = $1",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await
        .unwrap()
    }

    async fn keys(&self) -> Vec<(String, String)> {
        sqlx::query_as("SELECT keyset, tenant_id FROM seal_wrapped_key ORDER BY keyset")
            .fetch_all(&self.pool)
            .await
            .unwrap()
    }
}

fn bearer(title: &str, base_url: &str) -> CreateConnectionRequest {
    serde_json::from_value(json!({
        "title": title,
        "integrationId": "http_bearer",
        "connectionParameters": {"token": SECRET, "base_url": base_url},
    }))
    .unwrap()
}

async fn patch(
    service: &ConnectionService,
    id: &str,
    patch: Value,
) -> Result<runtara_connections::ConnectionDto, ServiceError> {
    let version = service
        .get_connection(id, TENANT)
        .await
        .unwrap()
        .edit_projection
        .expect("a connection has an edit projection")
        .version;
    let request: UpdateConnectionRequest = serde_json::from_value(json!({
        "version": version,
        "connectionParameterPatch": patch,
    }))
    .unwrap();
    service.update_connection(id, TENANT, request).await
}

fn is_validation(result: &Result<runtara_connections::ConnectionDto, ServiceError>) -> bool {
    matches!(result, Err(ServiceError::ValidationError(_)))
}

#[tokio::test]
async fn a_sealable_connection_is_stored_sealed_and_says_so() {
    let fixture = Fixture::new().await;
    let service = fixture.service(Some(fixture.sealed()));

    let id = service
        .create_connection(bearer("api", "https://api.example.com/v1"), TENANT)
        .await
        .unwrap();

    let (format, stored) = fixture.row(&id).await;
    assert_eq!(format, "seal");
    assert!(is_sealed(&stored["token"]));
    assert_eq!(stored["base_url"], "https://api.example.com/v1");
    assert!(!stored.to_string().contains(SECRET));

    // Whoever resolves the connection learns it is sealed, and gets the marker.
    let resolved = fixture
        .repository()
        .get_with_parameters(&id, TENANT)
        .await
        .unwrap()
        .unwrap();
    assert!(resolved.sealed);
    assert_eq!(resolved.connection_parameters.unwrap(), stored);

    // The editor sees a secret that is set, and never its envelope.
    let connection = service.get_connection(&id, TENANT).await.unwrap();
    let projection = connection.edit_projection.unwrap();
    assert!(projection.secret_state["token"].configured);
    let shown = serde_json::to_string(&projection.values).unwrap();
    assert!(!shown.contains(SEALED_MARKER) && !shown.contains(SECRET));

    // The tenant's key was made, and is the tenant's row.
    assert_eq!(
        fixture.keys().await,
        [(TENANT.to_owned(), TENANT.to_owned())]
    );
}

#[tokio::test]
async fn connections_that_are_not_sealed_stay_as_they_were() {
    let fixture = Fixture::new().await;

    // A destination seal cannot express (a base URL with a query), with
    // sealing on.
    let service = fixture.service(Some(fixture.sealed()));
    let id = service
        .create_connection(bearer("odd", "https://api.example.com/v1?tenant=1"), TENANT)
        .await
        .unwrap();
    let (format, stored) = fixture.row(&id).await;
    assert_eq!(format, "cipher");
    assert_eq!(stored["token"], SECRET);
    let resolved = fixture
        .repository()
        .get_with_parameters(&id, TENANT)
        .await
        .unwrap()
        .unwrap();
    assert!(!resolved.sealed);

    // A sealable type, with sealing off.
    let service = fixture.service(None);
    let id = service
        .create_connection(bearer("api", "https://api.example.com/v1"), TENANT)
        .await
        .unwrap();
    let (format, stored) = fixture.row(&id).await;
    assert_eq!(format, "cipher");
    assert_eq!(stored["token"], SECRET);

    // No connection was sealed, so no key was made.
    assert!(fixture.keys().await.is_empty());
}

/// Types with more than one secret, or whose secret has a second reader:
/// every secret they hold is sealed, and none is left readable in the row.
#[tokio::test]
async fn ai_provider_channel_and_mcp_connections_are_stored_sealed() {
    let fixture = Fixture::new().await;
    let service = fixture.service(Some(fixture.sealed()));
    let cases = [
        (
            "openai_api_key",
            json!({"api_key": SECRET}),
            vec!["api_key"],
        ),
        (
            "gemini_api_key",
            json!({"api_key": SECRET}),
            vec!["api_key"],
        ),
        (
            "slack_bot",
            json!({"bot_token": SECRET, "signing_secret": "signing-CANARY"}),
            vec!["bot_token", "signing_secret"],
        ),
        (
            "mailgun",
            json!({
                "api_key": SECRET,
                "domain": "mg.example.com",
                "webhook_signing_key": "signing-CANARY",
            }),
            vec!["api_key", "webhook_signing_key"],
        ),
        // A Mailgun connection with no signing key: only the API key.
        (
            "mailgun",
            json!({"api_key": SECRET, "domain": "mg.example.com"}),
            vec!["api_key"],
        ),
        (
            "mcp",
            json!({
                "url": "https://mcp.example.com/mcp",
                "auth_mode": "bearer",
                "bearer_token": SECRET,
            }),
            vec!["bearer_token"],
        ),
    ];
    for (index, (integration_id, parameters, sealed_fields)) in cases.into_iter().enumerate() {
        let request: CreateConnectionRequest = serde_json::from_value(json!({
            "title": format!("{integration_id} {index}"),
            "integrationId": integration_id,
            "connectionParameters": parameters,
        }))
        .unwrap();
        let id = service
            .create_connection(request, TENANT)
            .await
            .unwrap_or_else(|error| panic!("{integration_id}: {error:?}"));

        let (format, stored) = fixture.row(&id).await;
        assert_eq!(format, "seal", "{integration_id}");
        for field in &sealed_fields {
            assert!(is_sealed(&stored[*field]), "{integration_id}.{field}");
        }
        assert!(!stored.to_string().contains("CANARY"), "{integration_id}");
        // What a request presents is the secret that is sent, never one that
        // only verifies.
        let presented = runtara_connections::sealed::request_fields(integration_id, &stored);
        assert_eq!(presented, [sealed_fields[0]], "{integration_id}");
    }

    // An MCP server with no auth holds no secret, so its row is not sealed.
    let request: CreateConnectionRequest = serde_json::from_value(json!({
        "title": "open mcp",
        "integrationId": "mcp",
        "connectionParameters": {"url": "https://mcp.example.com/mcp", "auth_mode": "none"},
    }))
    .unwrap();
    let id = service.create_connection(request, TENANT).await.unwrap();
    assert_eq!(fixture.row(&id).await.0, "cipher");
}

#[tokio::test]
async fn an_edit_keeps_the_sealed_secret_unless_it_moves_the_destination() {
    let fixture = Fixture::new().await;
    let service = fixture.service(Some(fixture.sealed()));
    let id = service
        .create_connection(bearer("api", "https://api.example.com/v1"), TENANT)
        .await
        .unwrap();
    let (_, original) = fixture.row(&id).await;

    // Moving the base URL without the token is refused, and nothing changes.
    let moved = patch(
        &service,
        &id,
        json!({"set": {"base_url": "https://other.example.com"}}),
    )
    .await;
    assert!(is_validation(&moved), "{moved:?}");
    if let Err(ServiceError::ValidationError(message)) = &moved {
        assert!(message.contains("token"), "{message}");
        assert!(!message.contains(SECRET));
    }
    assert_eq!(fixture.row(&id).await.1, original);

    // With the token entered again, it is sealed for the new destination.
    patch(
        &service,
        &id,
        json!({"set": {"base_url": "https://other.example.com"}, "write": {"token": SECRET}}),
    )
    .await
    .unwrap();
    let (format, moved) = fixture.row(&id).await;
    assert_eq!(format, "seal");
    assert_eq!(moved["base_url"], "https://other.example.com");
    assert!(is_sealed(&moved["token"]));
    assert_ne!(moved["token"], original["token"]);
    assert!(!moved.to_string().contains(SECRET));

    // Entering only a new token keeps the destination and reseals.
    patch(&service, &id, json!({"write": {"token": "another-CANARY"}}))
        .await
        .unwrap();
    let (format, rotated) = fixture.row(&id).await;
    assert_eq!(format, "seal");
    assert_ne!(rotated["token"], moved["token"]);
    assert_eq!(
        rotated["token"][SEALED_MARKER]["policy"],
        moved["token"][SEALED_MARKER]["policy"]
    );
    assert!(!rotated.to_string().contains("another-CANARY"));
}

#[tokio::test]
async fn a_request_cannot_supply_a_sealed_values_marker() {
    let fixture = Fixture::new().await;
    let sealed = fixture.sealed();
    let service = fixture.service(Some(sealed));
    let id = service
        .create_connection(bearer("one", "https://api.example.com/v1"), TENANT)
        .await
        .unwrap();
    let other = service
        .create_connection(bearer("two", "https://api.example.com/v1"), TENANT)
        .await
        .unwrap();
    let (_, others) = fixture.row(&other).await;

    // On create, with sealing on or off.
    for service in [&service, &fixture.service(None)] {
        let request: CreateConnectionRequest = serde_json::from_value(json!({
            "title": "forged",
            "integrationId": "http_bearer",
            "connectionParameters": {"token": others["token"], "base_url": "https://api.example.com/v1"},
        }))
        .unwrap();
        let created = service.create_connection(request, TENANT).await;
        assert!(
            matches!(created, Err(ServiceError::ValidationError(_))),
            "{created:?}"
        );
    }

    // On update: another connection's marker in place of this one's.
    let forged = patch(&service, &id, json!({"write": {"token": others["token"]}})).await;
    assert!(is_validation(&forged), "{forged:?}");
}

#[tokio::test]
async fn two_instances_seal_a_tenants_connections_to_the_same_key() {
    let fixture = Fixture::new().await;
    let first = fixture.service(Some(fixture.sealed()));
    let second = fixture.service(Some(fixture.sealed()));

    let one = first
        .create_connection(bearer("one", "https://api.example.com/v1"), TENANT)
        .await
        .unwrap();
    let two = second
        .create_connection(bearer("two", "https://api.example.com/v1"), TENANT)
        .await
        .unwrap();
    let elsewhere = second
        .create_connection(
            bearer("three", "https://api.example.com/v1"),
            "auth0|globex",
        )
        .await
        .unwrap();

    let kid = |stored: &Value| stored["token"][SEALED_MARKER]["envelope"]["kid"].clone();
    let (_, one) = fixture.row(&one).await;
    let (_, two) = fixture.row(&two).await;
    assert_eq!(kid(&one), kid(&two));

    // Another tenant has a keyset of its own, named by its hash since its id
    // is not a name seal accepts, and its row is that tenant's.
    let (_, elsewhere) = fixture.row(&elsewhere).await;
    assert_ne!(kid(&elsewhere), kid(&one));
    let keys = fixture.keys().await;
    assert_eq!(keys.len(), 2);
    assert!(keys.contains(&(TENANT.to_owned(), TENANT.to_owned())));
    assert!(
        keys.iter()
            .any(|(keyset, tenant)| keyset.starts_with("t-") && tenant == "auth0|globex")
    );
}

/// Connections stored before sealing was on are sealed in place: each as it
/// would be on its next save, one tenant at a time or all, and only what
/// seal can hold. A dry run writes nothing.
#[tokio::test]
async fn existing_connections_are_sealed_in_place() {
    use runtara_connections::repository::connections::{SealingStats, refresh_token_guard};
    let fixture = Fixture::new().await;
    let unsealed = fixture.service(None);
    let sealable = unsealed
        .create_connection(bearer("api", "https://api.example.com/v1"), TENANT)
        .await
        .unwrap();
    // A destination seal cannot express: left as it is.
    let unexpressible = unsealed
        .create_connection(bearer("odd", "https://api.example.com/v1?tenant=1"), TENANT)
        .await
        .unwrap();
    let elsewhere = unsealed
        .create_connection(
            bearer("theirs", "https://api.example.com/v1"),
            "other-tenant",
        )
        .await
        .unwrap();
    // An authorized OAuth connection, with the rotation guard of its
    // readable refresh token, and a type seal does not handle.
    for (id, integration_id, parameters, guard) in [
        (
            "oauth-1",
            "http_oauth2_authorization_code",
            json!({
                "auth_url": "https://auth.example.com/authorize",
                "token_url": "https://auth.example.com/token",
                "base_url": "https://api.example.com",
                "client_id": "cid", "client_secret": "csec",
                "access_token": "at-CANARY", "refresh_token": "rt-CANARY",
            }),
            Some("guard-of-the-readable-token"),
        ),
        (
            "sftp-1",
            "sftp",
            json!({"host": "files.example.com", "password": SECRET}),
            None,
        ),
    ] {
        sqlx::query(
            "INSERT INTO connection_data_entity \
             (id, tenant_id, title, integration_id, connection_parameters, refresh_token_hash) \
             VALUES ($1, $2, $1, $3, $4, $5)",
        )
        .bind(id)
        .bind(TENANT)
        .bind(integration_id)
        .bind(parameters)
        .bind(guard)
        .execute(&fixture.pool)
        .await
        .unwrap();
    }
    let sealed = fixture.sealed();
    let repository = fixture.repository();

    // A dry run counts, and writes nothing.
    let would = repository
        .seal_existing(&sealed, Some(TENANT), true)
        .await
        .unwrap();
    assert_eq!(
        would,
        SealingStats {
            scanned: 4,
            sealed: 2,
            unchanged: 2,
            failed: 0
        }
    );
    for id in [sealable.as_str(), "oauth-1"] {
        assert_eq!(fixture.row(id).await.0, "cipher", "{id}");
    }

    // One tenant's connections.
    let stats = repository
        .seal_existing(&sealed, Some(TENANT), false)
        .await
        .unwrap();
    assert_eq!(stats, would);
    let (format, stored) = fixture.row(&sealable).await;
    assert_eq!(format, "seal");
    assert!(is_sealed(&stored["token"]));
    assert_eq!(stored["base_url"], "https://api.example.com/v1");
    let (format, oauth) = fixture.row("oauth-1").await;
    assert_eq!(format, "seal");
    for token in ["access_token", "refresh_token"] {
        assert!(is_sealed(&oauth[token]), "{token}");
    }
    assert!(!oauth.to_string().contains("CANARY"));
    // The rotation guard is now the sealed refresh token's, so that the next
    // rotation is stored.
    let guard: Option<String> = sqlx::query_scalar(
        "SELECT refresh_token_hash FROM connection_data_entity WHERE id = 'oauth-1'",
    )
    .fetch_one(&fixture.pool)
    .await
    .unwrap();
    assert_eq!(guard, refresh_token_guard(&oauth["refresh_token"]));
    assert!(guard.is_some());
    // What seal cannot hold stays readable, and stays as it was.
    for (id, field) in [(unexpressible.as_str(), "token"), ("sftp-1", "password")] {
        let (format, stored) = fixture.row(id).await;
        assert_eq!(format, "cipher", "{id}");
        assert_eq!(stored[field], SECRET, "{id}");
    }
    // Another tenant's connection was not touched.
    assert_eq!(fixture.row(&elsewhere).await.0, "cipher");
    assert!(
        fixture
            .repository()
            .get_with_parameters(&sealable, TENANT)
            .await
            .unwrap()
            .unwrap()
            .sealed
    );

    // Every tenant: only what is left to seal.
    let rest = repository
        .seal_existing(&sealed, None, false)
        .await
        .unwrap();
    assert_eq!(
        rest,
        SealingStats {
            scanned: 3,
            sealed: 1,
            unchanged: 2,
            failed: 0
        }
    );
    assert_eq!(fixture.row(&elsewhere).await.0, "seal");
    // Run again, nothing changes: the secrets already sealed are not looked at.
    let again = repository
        .seal_existing(&sealed, None, false)
        .await
        .unwrap();
    assert_eq!((again.scanned, again.sealed), (2, 0));
    // Each tenant's secrets are sealed to its own key.
    assert_eq!(
        fixture.keys().await,
        [
            (TENANT.to_owned(), TENANT.to_owned()),
            ("other-tenant".to_owned(), "other-tenant".to_owned()),
        ]
    );
}

/// A tenant that leaves has its seal keys destroyed: what was sealed for it
/// can no longer be opened, here or from any copy of its rows, and no other
/// tenant is touched.
#[tokio::test]
async fn a_leaving_tenants_keys_are_destroyed_and_its_secrets_with_them() {
    let fixture = Fixture::new().await;
    let sealed = fixture.sealed();
    let service = fixture.service(Some(sealed.clone()));
    let leaving = service
        .create_connection(bearer("api", "https://api.example.com/v1"), TENANT)
        .await
        .unwrap();
    let staying = service
        .create_connection(
            bearer("theirs", "https://api.example.com/v1"),
            "other-tenant",
        )
        .await
        .unwrap();
    assert_eq!(fixture.keys().await.len(), 2);
    let (_, stored_leaving) = fixture.row(&leaving).await;
    let (_, stored_staying) = fixture.row(&staying).await;
    // Both open before: a request to a host that does not exist gets as far
    // as the network, not refused for its secret.
    let attempt = |tenant: &'static str, connection: String, stored: Value| {
        let sealed = sealed.clone();
        async move {
            sealed
                .send_bounded(
                    tenant,
                    &connection,
                    "http_bearer",
                    &stored,
                    "GET",
                    "https://api.example.com/v1/orders",
                    [],
                    None,
                    1024,
                )
                .await
        }
    };
    let code = |outcome: Result<_, runtara_connections::sealed::SendError>| match outcome {
        Err(runtara_connections::sealed::SendError::Seal(error)) => error.code.as_str().to_owned(),
        Ok(_) => "sent".to_owned(),
        Err(other) => format!("{other:?}"),
    };

    sealed.destroy_tenant_keys(TENANT).await.unwrap();

    assert_eq!(
        fixture.keys().await,
        [("other-tenant".to_owned(), "other-tenant".to_owned())]
    );
    // The leaving tenant's secret no longer opens, at once and for good: a
    // new instance over the same table cannot open it either.
    for seal in [sealed.clone(), fixture.sealed()] {
        let outcome = seal
            .send_bounded(
                TENANT,
                &leaving,
                "http_bearer",
                &stored_leaving,
                "GET",
                "https://api.example.com/v1/orders",
                [],
                None,
                1024,
            )
            .await;
        let refused = code(outcome);
        assert!(
            ["KEYSET_UNKNOWN", "OPEN_FAILED", "KEYS_UNAVAILABLE"].contains(&refused.as_str()),
            "{refused}"
        );
    }
    // The other tenant's still does: it fails, if at all, on the network.
    let other = code(attempt("other-tenant", staying, stored_staying).await);
    assert!(
        ![
            "KEYSET_UNKNOWN",
            "OPEN_FAILED",
            "KEYS_UNAVAILABLE",
            "POLICY_DENIED"
        ]
        .contains(&other.as_str()),
        "{other}"
    );
    // Destroying what is already gone is not an error.
    sealed.destroy_tenant_keys(TENANT).await.unwrap();
}
