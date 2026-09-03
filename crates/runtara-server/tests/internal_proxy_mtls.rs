//! End-to-end coverage for the narrow `http_mtls` connection type.
//!
//! This starts a real Rustls server that refuses unauthenticated clients, then
//! calls the proxy core with a persisted connection. A 200 proves all of the
//! security-critical seams: form/save validation, PEM parsing, custom CA trust,
//! client-certificate presentation, and selection of the mTLS reqwest client.

use std::collections::HashMap;
use std::sync::{Arc, Once};
use std::time::Duration;

use rcgen::{
    BasicConstraints, CertificateParams, CertifiedIssuer, ExtendedKeyUsagePurpose, IsCa, KeyPair,
    KeyUsagePurpose,
};
use runtara_connections::crypto::noop::NoOpCipher;
use runtara_connections::repository::connections::ConnectionRepository;
use runtara_connections::service::connections::{ConnectionService, ServiceError};
use runtara_connections::{
    ConnectionsConfig, ConnectionsFacade, ConnectionsState, CreateConnectionRequest,
    IntegrationCompatibility,
};
use runtara_server::api::handlers::internal_proxy::{ProxyRequest, execute_proxy_request};
use runtara_server::config::Config;
use serde_json::json;
use sqlx::PgPool;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::{
    RootCertStore, ServerConfig, pki_types::PrivateKeyDer, server::WebPkiClientVerifier,
};
use uuid::Uuid;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

fn database_url() -> String {
    std::env::var("TEST_RUNTARA_SERVER_DATABASE_URL")
        .or_else(|_| std::env::var("RUNTARA_SERVER_DATABASE_URL"))
        .expect("db-integration-tests requires TEST_RUNTARA_SERVER_DATABASE_URL or RUNTARA_SERVER_DATABASE_URL")
}

async fn test_pool() -> PgPool {
    let pool = PgPool::connect(&database_url())
        .await
        .expect("required server test database must accept connections");
    MIGRATOR
        .run(&pool)
        .await
        .expect("required server migrations must succeed");
    pool
}

fn allow_local_egress_for_this_test_binary() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        // Both the proxy's preflight gate and the DNS-guarded outbound client
        // must allow the local test TLS server. This is a separate test binary,
        // so the read-once production guards cannot leak elsewhere.
        std::env::set_var("RUNTARA_CONNECTION_ALLOW_HTTP_HOSTS", "127.0.0.1");
        std::env::set_var("RUNTARA_PROXY_ALLOWED_HOSTS", "127.0.0.1");
        std::env::set_var("TENANT_ID", "internal_proxy_mtls_test");
        std::env::set_var("OBJECT_MODEL_DATABASE_URL", database_url());
        std::env::set_var("RUNTARA_MCP_SESSION_STORE", "local");
        std::env::set_var("ADAPTIVE_RATE_LIMITING", "false");
    });
}

fn init_server_config() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        runtara_server::config::init(Config::from_env().expect("build test server configuration"));
    });
}

fn compatibility() -> Arc<IntegrationCompatibility> {
    Arc::new(IntegrationCompatibility::new(HashMap::new()))
}

fn facade(pool: PgPool, compatibility: Arc<IntegrationCompatibility>) -> ConnectionsFacade {
    ConnectionsFacade::new(ConnectionsState::from_config(ConnectionsConfig {
        db_pool: pool,
        redis_manager: None,
        public_base_url: "http://localhost".to_string(),
        http_client: runtara_connections::net::build_hardened_client(),
        cipher: Arc::new(NoOpCipher),
        compatibility,
        agent_catalog: Arc::new(runtara_dsl::agent_meta::AgentCatalog::from_agents(
            Vec::new(),
        )),
        connection_events: None,
    }))
}

struct MtlsFixture {
    base_url: String,
    client_certificate_pem: String,
    client_private_key_pem: String,
    server_ca_pem: String,
    server: tokio::task::JoinHandle<()>,
}

async fn mtls_fixture() -> MtlsFixture {
    let mut ca_params = CertificateParams::new(Vec::<String>::new()).expect("CA params");
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyCertSign,
    ];
    let ca = CertifiedIssuer::self_signed(ca_params, KeyPair::generate().expect("CA key"))
        .expect("self-signed CA");

    let mut server_params =
        CertificateParams::new(vec!["127.0.0.1".to_string()]).expect("server params");
    server_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let server_key = KeyPair::generate().expect("server key");
    let server_cert = server_params
        .signed_by(&server_key, &ca)
        .expect("server certificate");

    let mut client_params = CertificateParams::new(Vec::<String>::new()).expect("client params");
    client_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
    let client_key = KeyPair::generate().expect("client key");
    let client_cert = client_params
        .signed_by(&client_key, &ca)
        .expect("client certificate");

    let mut roots = RootCertStore::empty();
    roots.add(ca.der().clone()).expect("add test CA");
    let verifier = WebPkiClientVerifier::builder(Arc::new(roots))
        .build()
        .expect("require a client certificate");
    let server_config = ServerConfig::builder()
        .with_client_cert_verifier(verifier)
        .with_single_cert(
            vec![server_cert.der().clone()],
            PrivateKeyDer::Pkcs8(server_key.serialize_der().into()),
        )
        .expect("server TLS configuration");
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind TLS server");
    let address = listener.local_addr().expect("TLS server address");
    let acceptor = TlsAcceptor::from(Arc::new(server_config));
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept TLS connection");
        let mut stream = acceptor.accept(stream).await.expect("mTLS handshake");
        let mut request = [0_u8; 4096];
        let bytes = stream.read(&mut request).await.expect("read HTTP request");
        assert!(
            std::str::from_utf8(&request[..bytes])
                .expect("HTTP request text")
                .starts_with("GET /whoami HTTP/1.1"),
            "proxy should pin the relative request under the configured base URL"
        );
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 13\r\nconnection: close\r\n\r\n{\"mtls\":true}",
            )
            .await
            .expect("write HTTP response");
        stream.flush().await.expect("flush HTTP response");
    });

    MtlsFixture {
        base_url: format!("https://127.0.0.1:{}", address.port()),
        client_certificate_pem: client_cert.pem(),
        client_private_key_pem: client_key.serialize_pem(),
        server_ca_pem: ca.pem(),
        server,
    }
}

#[tokio::test]
async fn proxy_uses_persisted_http_mtls_identity_and_private_server_ca() {
    allow_local_egress_for_this_test_binary();
    init_server_config();
    let pool = test_pool().await;
    let tenant_id = format!("mtls-{}", Uuid::new_v4());
    let MtlsFixture {
        base_url,
        client_certificate_pem,
        client_private_key_pem,
        server_ca_pem,
        server,
    } = mtls_fixture().await;
    let compatibility = compatibility();
    let service = ConnectionService::new(
        Arc::new(ConnectionRepository::new(
            pool.clone(),
            Arc::new(NoOpCipher),
        )),
        compatibility.clone(),
    );

    // The create path exercises the exact public descriptor and save-time
    // Rustls build. A syntactically valid but mismatched certificate/key pair
    // must fail before it can be persisted.
    let mismatched = service
        .create_connection(
            CreateConnectionRequest {
                title: format!("mTLS mismatch {tenant_id}"),
                connection_subtype: None,
                connection_parameters: Some(json!({
                    "base_url": base_url.clone(),
                    "client_certificate_pem": client_certificate_pem.clone(),
                    "client_private_key_pem": KeyPair::generate()
                        .expect("different client key")
                        .serialize_pem(),
                    "server_ca_pem": server_ca_pem.clone(),
                })),
                integration_id: Some("http_mtls".to_string()),
                rate_limit_config: None,
                valid_until: None,
                is_default_file_storage: None,
                default_for: None,
            },
            &tenant_id,
        )
        .await;
    let mismatch_error = mismatched.expect_err("mismatched mTLS key must be rejected");
    let ServiceError::ValidationError(mismatch_message) = mismatch_error else {
        panic!("certificate/key mismatch should be a validation error");
    };
    assert!(
        mismatch_message.contains("mTLS client could not be initialized"),
        "certificate/key mismatch should fail while building Rustls config"
    );

    let connection_id = service
        .create_connection(
            CreateConnectionRequest {
                title: format!("mTLS valid {tenant_id}"),
                connection_subtype: None,
                connection_parameters: Some(json!({
                    "base_url": base_url,
                    "client_certificate_pem": client_certificate_pem,
                    "client_private_key_pem": client_private_key_pem,
                    "server_ca_pem": server_ca_pem,
                })),
                integration_id: Some("http_mtls".to_string()),
                rate_limit_config: None,
                valid_until: None,
                is_default_file_storage: None,
                default_for: None,
            },
            &tenant_id,
        )
        .await
        .expect("create valid mTLS connection");

    let facade = facade(pool.clone(), compatibility);
    let proxy_client = runtara_connections::net::build_hardened_client();
    let (status, response) = execute_proxy_request(
        &tenant_id,
        &facade,
        &proxy_client,
        ProxyRequest {
            method: "GET".to_string(),
            url: "/whoami".to_string(),
            headers: HashMap::new(),
            body: None,
            body_raw: None,
            connection_id: Some(connection_id.clone()),
            ai_provider: None,
            aws_service: None,
            endpoint_ref: None,
            endpoint: None,
            timeout_ms: Some(5_000),
        },
    )
    .await
    .expect("proxy mTLS request");
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(response.0.status, 200);
    assert_eq!(response.0.body, Some(json!({"mtls": true})));

    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("TLS server should receive one request")
        .expect("TLS server task should succeed");

    // The proxy records analytics asynchronously; let that best-effort task
    // reach the database before removing this test's isolated rows.
    tokio::task::yield_now().await;
    let _ = sqlx::query("DELETE FROM rate_limit_events WHERE tenant_id = $1")
        .bind(&tenant_id)
        .execute(&pool)
        .await;
    let _ = sqlx::query("DELETE FROM connection_defaults WHERE tenant_id = $1")
        .bind(&tenant_id)
        .execute(&pool)
        .await;
    let _ = sqlx::query("DELETE FROM connection_data_entity WHERE id = $1 AND tenant_id = $2")
        .bind(&connection_id)
        .bind(&tenant_id)
        .execute(&pool)
        .await;
}
