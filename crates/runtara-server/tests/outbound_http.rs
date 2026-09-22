//! Native outbound service coverage with persisted connections and loopback providers.
//!
//! This starts a real Rustls server that refuses unauthenticated clients, then
//! calls the native outbound service with a persisted connection. A 200 proves all of the
//! security-critical seams: form/save validation, PEM parsing, custom CA trust,
//! client-certificate presentation, and selection of the mTLS reqwest client.

use std::collections::HashMap;
use std::sync::{Arc, Once};
use std::time::Duration;

use rcgen::{
    BasicConstraints, CertificateParams, CertifiedIssuer, ExtendedKeyUsagePurpose, IsCa, KeyPair,
    KeyUsagePurpose,
};
use runtara_component_host::outbound_http::{
    ConnectionDestination, Destination, OutboundContext, OutboundHttpHost, RequestOptions,
};
use runtara_connections::crypto::noop::NoOpCipher;
use runtara_connections::repository::connections::ConnectionRepository;
use runtara_connections::service::connections::{ConnectionService, ServiceError};
use runtara_connections::{
    ConnectionsConfig, ConnectionsFacade, ConnectionsState, CreateConnectionRequest,
    IntegrationCompatibility,
};
use runtara_server::api::services::outbound_http::NativeOutboundHttp;
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
        // Both the service's preflight gate and the DNS-guarded outbound client
        // must allow the local test TLS server. This is a separate test binary,
        // so the read-once production guards cannot leak elsewhere.
        std::env::set_var("RUNTARA_CONNECTION_ALLOW_HTTP_HOSTS", "127.0.0.1");
        std::env::set_var("RUNTARA_PROXY_ALLOWED_HOSTS", "127.0.0.1");
        std::env::set_var("RUNTARA_PROXY_ALLOW_HTTP_HOSTS", "127.0.0.1");
        std::env::set_var("TENANT_ID", "native_outbound_test");
        std::env::set_var("OBJECT_MODEL_DATABASE_URL", database_url());
        std::env::set_var("RUNTARA_MCP_SESSION_STORE", "local");
        std::env::set_var("ADAPTIVE_RATE_LIMITING", "true");
        std::env::set_var("RUNTARA_ENDPOINT_REF_SECRET", "synthetic-outbound-ref-key");
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
    facade_with_redis(pool, compatibility, None)
}

fn facade_with_redis(
    pool: PgPool,
    compatibility: Arc<IntegrationCompatibility>,
    redis_manager: Option<redis::aio::ConnectionManager>,
) -> ConnectionsFacade {
    ConnectionsFacade::new(ConnectionsState::from_config(ConnectionsConfig {
        db_pool: pool,
        redis_manager,
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
async fn outbound_uses_persisted_http_mtls_identity_and_private_server_ca() {
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
    let outbound = NativeOutboundHttp {
        facade: Arc::new(facade),
        client: runtara_connections::net::build_hardened_client(),
    };
    let response = outbound
        .request(
            &OutboundContext {
                tenant_id: tenant_id.clone(),
                instance_id: None,
            },
            RequestOptions {
                method: "GET".into(),
                destination: Destination::Connection(ConnectionDestination {
                    connection_id: connection_id.clone(),
                    url: "/whoami".into(),
                    ai_provider: None,
                    aws_service: None,
                    endpoint_ref: None,
                    endpoint: None,
                }),
                headers: Vec::new(),
                body: None,
                timeout_ms: Some(5_000),
                max_response_bytes: None,
            },
        )
        .await
        .expect("native outbound mTLS request");
    assert_eq!(response.status, 200);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&response.body).unwrap(),
        json!({"mtls": true})
    );

    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("TLS server should receive one request")
        .expect("TLS server task should succeed");

    // The outbound service records analytics asynchronously; let that best-effort task
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

/// Only the mock provider listens on HTTP. The WASM-to-host boundary is native.
#[cfg(feature = "component-integration-tests")]
#[tokio::test(flavor = "multi_thread")]
async fn real_http_component_uses_stored_credentials_without_internal_listener()
-> anyhow::Result<()> {
    use axum::{
        Router,
        body::Bytes,
        http::{HeaderMap, StatusCode},
        routing::post,
    };
    use runtara_component_host::{
        ComponentDispatcherService, DispatcherEnv, ResolvedConnection, TestCapabilityRequest,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    allow_local_egress_for_this_test_binary();
    init_server_config();
    let pool = test_pool().await;
    let tenant = format!("outbound-{}", Uuid::new_v4());
    let seen = Arc::new(AtomicUsize::new(0));
    let app = Router::new().route(
        "/api/echo",
        post({
            let seen = seen.clone();
            move |headers: HeaderMap, body: Bytes| {
                let seen = seen.clone();
                async move {
                    assert_eq!(
                        headers.get("authorization").unwrap(),
                        "Bearer synthetic-outbound-fixture"
                    );
                    assert!(headers.get("x-runtara-connection-id").is_none());
                    assert_eq!(body.as_ref(), br#"{"hello":"world"}"#);
                    seen.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::CREATED,
                        [
                            ("content-type", "application/octet-stream"),
                            ("x-provider", "fixture"),
                        ],
                        vec![0u8, 255, 1, 128],
                    )
                }
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let base_url = format!("http://{}/api", listener.local_addr()?);
    let server = tokio::spawn(async move { axum::serve(listener, app).await });
    let compatibility = compatibility();
    let connections = ConnectionService::new(
        Arc::new(ConnectionRepository::new(
            pool.clone(),
            Arc::new(NoOpCipher),
        )),
        compatibility.clone(),
    );
    let connection_id = connections
        .create_connection(
            CreateConnectionRequest {
                title: format!("Synthetic outbound fixture {tenant}"),
                connection_subtype: None,
                connection_parameters: Some(
                    json!({"base_url":base_url, "token":"synthetic-outbound-fixture"}),
                ),
                integration_id: Some("http_bearer".into()),
                rate_limit_config: None,
                valid_until: None,
                is_default_file_storage: None,
                default_for: None,
            },
            &tenant,
        )
        .await
        .expect("create synthetic bearer connection");
    let outbound = Arc::new(NativeOutboundHttp {
        facade: Arc::new(facade(pool.clone(), compatibility)),
        client: runtara_connections::net::build_hardened_client(),
    });
    let bundle = tempfile::tempdir()?;
    let components = std::env::var_os("RUNTARA_AGENT_COMPONENTS_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/wasm32-wasip2/release")
        });
    for extension in ["wasm", "meta.json"] {
        let name = format!("runtara_agent_http.{extension}");
        std::fs::copy(components.join(&name), bundle.path().join(name))?;
    }
    let dispatcher = ComponentDispatcherService::from_dir(
        bundle.path(),
        DispatcherEnv {
            core_http_url: String::new(),
        },
    )
    .await?;
    dispatcher.set_outbound_http(outbound.clone())?;
    let request = |tenant_id: String| TestCapabilityRequest {
        tenant_id,
        agent_id: "http".into(),
        capability_id: "http-request".into(),
        input: json!({"url":"/echo", "method":"POST", "body":{"hello":"world"}, "response_type":"binary", "headers":{"authorization":"guest-value", "AUTHORIZATION":"guest-case-variant", "X-Org-Id":tenant, "X-Runtara-Connection-Id":"forged-connection"}}),
        connection: Some(ResolvedConnection {
            connection_id: connection_id.clone(),
            integration_id: "http_bearer".into(),
            connection_subtype: None,
            parameters: json!({}),
            rate_limit_config: None,
        }),
    };
    let result = dispatcher
        .test_capability(request("different-tenant".into()))
        .await?;
    assert_eq!(result.error.unwrap().code, "HTTP_4XX");
    assert_eq!(
        seen.load(Ordering::SeqCst),
        0,
        "wrong tenant must not reach the provider"
    );
    let result = dispatcher.test_capability(request(tenant.clone())).await?;
    assert!(result.success, "{:?}", result.error);
    let output = result.output.unwrap();
    assert_eq!(output["status_code"], 201);
    assert_eq!(output["headers"]["x-provider"], "fixture");
    assert_eq!(output["body"]["base64"], "AP8BgA==");
    assert_eq!(seen.load(Ordering::SeqCst), 1);
    server.abort();
    Ok(())
}

fn public_request(url: String, timeout_ms: u64) -> RequestOptions {
    RequestOptions {
        destination: Destination::Public(url),
        method: "GET".into(),
        headers: vec![],
        body: None,
        timeout_ms: Some(timeout_ms),
        max_response_bytes: None,
    }
}

#[tokio::test]
async fn public_signed_urls_statuses_body_deadlines_and_client_reuse() -> anyhow::Result<()> {
    use axum::{
        Router,
        body::Body,
        http::{HeaderMap, StatusCode, Uri},
        routing::get,
    };
    allow_local_egress_for_this_test_binary();
    init_server_config();
    let app = Router::new()
        .route(
            "/signed",
            get(|uri: Uri, headers: HeaderMap| async move {
                assert_eq!(uri.query(), Some("signature=a%2Fb%2Bc&x=1&x=2"));
                assert!(headers.get("authorization").is_none());
                (
                    StatusCode::TOO_MANY_REQUESTS,
                    [("retry-after", "2")],
                    vec![0u8, 255, 1],
                )
            }),
        )
        .route(
            "/slow-body",
            get(|| async {
                Body::from_stream(futures::stream::unfold(0, |index| async move {
                    if index == 0 {
                        Some((Ok::<_, std::io::Error>("first"), 1))
                    } else if index == 1 {
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        Some((Ok("last"), 2))
                    } else {
                        None
                    }
                }))
            }),
        );
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    let server = tokio::spawn(async move { axum::serve(listener, app).await });
    let outbound = NativeOutboundHttp {
        facade: Arc::new(facade(test_pool().await, compatibility())),
        client: runtara_connections::net::build_hardened_client(),
    };
    let context = OutboundContext {
        tenant_id: "public-fixture".into(),
        instance_id: Some("instance-fixture".into()),
    };
    let error = outbound
        .request(&context, public_request(format!("{base}/slow-body"), 50))
        .await
        .unwrap_err();
    assert_eq!(error.code, "HTTP_DEADLINE_EXCEEDED");
    let response = outbound
        .request(
            &context,
            public_request(format!("{base}/signed?signature=a%2Fb%2Bc&x=1&x=2"), 1_000),
        )
        .await
        .unwrap();
    assert_eq!(response.status, 429);
    assert_eq!(response.body, [0, 255, 1]);
    assert!(
        response
            .headers
            .contains(&("retry-after".into(), "2".into()))
    );
    server.abort();
    Ok(())
}

#[tokio::test]
async fn request_deadline_includes_waiting_for_connection_lookup() -> anyhow::Result<()> {
    allow_local_egress_for_this_test_binary();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url())
        .await?;
    let held_connection = pool.acquire().await?;
    let outbound = NativeOutboundHttp {
        facade: Arc::new(facade(pool.clone(), compatibility())),
        client: runtara_connections::net::build_hardened_client(),
    };
    let context = OutboundContext {
        tenant_id: "lookup-fixture".into(),
        instance_id: None,
    };
    let request = || RequestOptions {
        destination: Destination::Connection(ConnectionDestination {
            connection_id: "absent-fixture".into(),
            url: "/unused".into(),
            endpoint: None,
            endpoint_ref: None,
            ai_provider: None,
            aws_service: None,
        }),
        method: "GET".into(),
        headers: vec![],
        body: None,
        timeout_ms: Some(50),
        max_response_bytes: None,
    };
    let error = outbound.request(&context, request()).await.unwrap_err();
    assert_eq!(error.code, "HTTP_DEADLINE_EXCEEDED");
    drop(held_connection);
    let error = outbound.request(&context, request()).await.unwrap_err();
    assert_eq!(
        error.code, "CONNECTION_NOT_FOUND",
        "cancelled lookup must release its pool wait"
    );
    Ok(())
}

async fn synthetic_connection(
    pool: &PgPool,
    tenant: &str,
    integration: &str,
    parameters: serde_json::Value,
) -> anyhow::Result<String> {
    ConnectionService::new(
        Arc::new(ConnectionRepository::new(
            pool.clone(),
            Arc::new(NoOpCipher),
        )),
        compatibility(),
    )
    .create_connection(
        CreateConnectionRequest {
            title: format!("Outbound {integration} {}", Uuid::new_v4()),
            integration_id: Some(integration.into()),
            connection_subtype: None,
            connection_parameters: Some(parameters),
            rate_limit_config: None,
            valid_until: None,
            is_default_file_storage: None,
            default_for: None,
        },
        tenant,
    )
    .await
    .map_err(|_| anyhow::anyhow!("synthetic connection creation failed"))
}

fn connection_request(connection: &str, path: &str) -> RequestOptions {
    RequestOptions {
        destination: Destination::Connection(ConnectionDestination {
            connection_id: connection.into(),
            url: path.into(),
            endpoint: None,
            endpoint_ref: None,
            ai_provider: None,
            aws_service: None,
        }),
        method: "GET".into(),
        headers: vec![],
        body: None,
        timeout_ms: Some(5_000),
        max_response_bytes: None,
    }
}

#[tokio::test]
async fn native_destination_controls_pin_urls_and_verify_endpoint_bindings() -> anyhow::Result<()> {
    use runtara_server::api::services::endpoint_ref::{EndpointBinding, EndpointRefKeyring, sign};
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    allow_local_egress_for_this_test_binary();
    init_server_config();
    let pool = test_pool().await;
    let provider = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/bound/v3/conversations/fixture/activities"))
        .and(header("authorization", "Bearer synthetic-pin-token"))
        .respond_with(ResponseTemplate::new(200).set_body_string("bound"))
        .expect(1)
        .mount(&provider)
        .await;
    let tenant = format!("pin-outbound-{}", Uuid::new_v4());
    let connection = synthetic_connection(
        &pool,
        &tenant,
        "http_bearer",
        json!({"base_url":format!("{}/original", provider.uri()), "token":"synthetic-pin-token"}),
    )
    .await?;
    let outbound = NativeOutboundHttp {
        facade: Arc::new(facade(pool.clone(), compatibility())),
        client: runtara_connections::net::build_hardened_client(),
    };
    let context = OutboundContext {
        tenant_id: tenant.clone(),
        instance_id: None,
    };
    // An absolute URL outside the connection's base path must not reach even
    // an otherwise allowed provider. An undeclared endpoint must not fall back.
    let denied = outbound
        .request(
            &context,
            connection_request(&connection, &format!("{}/outside", provider.uri())),
        )
        .await
        .unwrap_err();
    assert_eq!(denied.status, Some(403));
    let mut named = connection_request(&connection, "/resource");
    if let Destination::Connection(ref mut target) = named.destination {
        target.endpoint = Some("undeclared".into());
    }
    assert_eq!(
        outbound.request(&context, named).await.unwrap_err().code,
        "NAMED_ENDPOINT_REJECTED"
    );

    let keyring = EndpointRefKeyring::new("1", b"synthetic-outbound-ref-key".to_vec());
    let mut binding = EndpointBinding {
        v: EndpointBinding::CURRENT_VERSION,
        tenant_id: tenant,
        connection_id: connection.clone(),
        base_url: format!("{}/bound/", provider.uri()),
        conversation_id: Some("fixture".into()),
        conversation_type: None,
        ms_tenant_id: None,
        iat: 1_700_000_000,
    };
    let bound = |binding: &EndpointBinding, target_path: &str| {
        let mut request = connection_request(&connection, target_path);
        if let Destination::Connection(ref mut target) = request.destination {
            target.endpoint_ref = Some(sign(&keyring, binding));
        }
        request
    };
    let target_path = "/v3/conversations/fixture/activities";
    binding.tenant_id = "foreign-fixture-tenant".into();
    assert_eq!(
        outbound
            .request(&context, bound(&binding, target_path))
            .await
            .unwrap_err()
            .status,
        Some(403)
    );
    binding.tenant_id.clone_from(&context.tenant_id);
    assert_eq!(
        outbound
            .request(
                &context,
                bound(&binding, "/v3/conversations/other/activities")
            )
            .await
            .unwrap_err()
            .status,
        Some(403)
    );
    let response = outbound
        .request(&context, bound(&binding, target_path))
        .await
        .unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(response.body, b"bound");
    assert_eq!(provider.received_requests().await.unwrap().len(), 1);
    provider.verify().await;
    Ok(())
}

#[tokio::test]
async fn native_outbound_refreshes_oauth_and_persists_rotation_before_reuse() -> anyhow::Result<()>
{
    use wiremock::matchers::{body_string_contains, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    allow_local_egress_for_this_test_binary();
    init_server_config();
    let pool = test_pool().await;
    let provider = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .and(body_string_contains("grant_type=refresh_token"))
        .and(body_string_contains("refresh_token=synthetic-old-refresh"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token":"synthetic-new-access", "refresh_token":"synthetic-new-refresh",
            "token_type":"Bearer", "expires_in":3600
        })))
        .expect(1)
        .mount(&provider)
        .await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(200)
            .set_delay(Duration::from_secs(2))
            .set_body_json(json!({"access_token":"synthetic-unused", "token_type":"Bearer", "expires_in":3600})))
        .with_priority(1).up_to_n_times(1).expect(1).mount(&provider).await;
    Mock::given(method("GET"))
        .and(path("/api/resource"))
        .and(header("authorization", "Bearer synthetic-new-access"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"authorized".to_vec()))
        .expect(2)
        .mount(&provider)
        .await;
    let tenant = format!("oauth-outbound-{}", Uuid::new_v4());
    let connection = synthetic_connection(
        &pool,
        &tenant,
        "http_oauth2_authorization_code",
        json!({
            "base_url":format!("{}/api", provider.uri()),
            "auth_url":format!("{}/oauth/authorize", provider.uri()),
            "token_url":format!("{}/oauth/token", provider.uri()),
            "client_id":"synthetic-client", "client_secret":"synthetic-client-secret",
            "token_auth":"basic"
        }),
    )
    .await?;
    sqlx::query("UPDATE connection_data_entity SET connection_parameters=connection_parameters || $1, status='ACTIVE' WHERE id=$2")
        .bind(json!({"access_token":"synthetic-expired-access", "refresh_token":"synthetic-old-refresh", "token_expires_at":"2020-01-01T00:00:00Z"}))
        .bind(&connection).execute(&pool).await?;
    let outbound = NativeOutboundHttp {
        facade: Arc::new(facade(pool.clone(), compatibility())),
        client: runtara_connections::net::build_hardened_client(),
    };
    let context = OutboundContext {
        tenant_id: tenant,
        instance_id: Some("oauth-fixture".into()),
    };
    let mut timed = connection_request(&connection, "/resource");
    timed.timeout_ms = Some(50);
    let failed = outbound.request(&context, timed).await.unwrap_err();
    assert_eq!(failed.code, "HTTP_DEADLINE_EXCEEDED");
    let after_cancel: serde_json::Value =
        sqlx::query_scalar("SELECT connection_parameters FROM connection_data_entity WHERE id=$1")
            .bind(&connection)
            .fetch_one(&pool)
            .await?;
    assert!(after_cancel["access_token"] == "synthetic-expired-access");
    for _ in 0..2 {
        let response = outbound
            .request(&context, connection_request(&connection, "/resource"))
            .await
            .unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"authorized");
    }
    // Only synthetic values created above are read; never print stored parameters.
    let stored: serde_json::Value =
        sqlx::query_scalar("SELECT connection_parameters FROM connection_data_entity WHERE id=$1")
            .bind(connection)
            .fetch_one(&pool)
            .await?;
    assert!(stored["access_token"] == "synthetic-new-access");
    assert!(stored["refresh_token"] == "synthetic-new-refresh");
    assert!(stored["token_expires_at"] != "2020-01-01T00:00:00Z");
    provider.verify().await;
    Ok(())
}

#[tokio::test]
async fn native_outbound_signs_exact_binary_payload_for_aws_and_azure() -> anyhow::Result<()> {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use sha2::{Digest, Sha256};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    allow_local_egress_for_this_test_binary();
    init_server_config();
    let pool = test_pool().await;
    let provider = MockServer::start().await;
    let payload = vec![0, 255, 1, 128, 10];
    let tenant = format!("signing-outbound-{}", Uuid::new_v4());
    let context = OutboundContext {
        tenant_id: tenant.clone(),
        instance_id: None,
    };
    let outbound = NativeOutboundHttp {
        facade: Arc::new(facade(pool.clone(), compatibility())),
        client: runtara_connections::net::build_hardened_client(),
    };
    let aws = synthetic_connection(
        &pool,
        &tenant,
        "s3_compatible",
        json!({
            "endpoint":provider.uri(), "access_key_id":"synthetic-access",
            "secret_access_key":"synthetic-secret", "region":"us-east-1"
        }),
    )
    .await?;
    let azure = synthetic_connection(&pool, &tenant, "azure_blob_storage", json!({
        "endpoint_override":provider.uri(), "account_name":"fixture", "account_key":STANDARD.encode(b"synthetic-account-key")
    })).await?;
    for (connection, destination) in [(&aws, "/bucket/file.bin"), (&azure, "/container/file.bin")] {
        let expected_path = if connection == &aws {
            destination.into()
        } else {
            format!("/fixture{destination}")
        };
        Mock::given(method("PUT"))
            .and(path(expected_path))
            .respond_with(ResponseTemplate::new(201))
            .expect(1)
            .mount(&provider)
            .await;
        let mut request = connection_request(connection, destination);
        request.method = "PUT".into();
        request.body = Some(payload.clone());
        request.headers = vec![
            ("content-type".into(), "application/octet-stream".into()),
            ("Authorization".into(), "guest-forged".into()),
            ("X-Amz-Date".into(), "guest-forged".into()),
            ("X-Ms-Date".into(), "guest-forged".into()),
        ];
        let response = outbound.request(&context, request).await.unwrap();
        assert_eq!(response.status, 201);
    }
    let received = provider.received_requests().await.unwrap();
    assert_eq!(received.len(), 2);
    for request in &received {
        assert_eq!(request.body, payload);
        assert_eq!(request.headers.get_all("authorization").iter().count(), 1);
    }
    let aws_request = &received[0];
    let authorization = aws_request.headers["authorization"].to_str()?;
    assert!(authorization.starts_with("AWS4-HMAC-SHA256 Credential=synthetic-access/"));
    assert!(authorization.contains("/us-east-1/s3/aws4_request"));
    assert_eq!(
        aws_request.headers["x-amz-content-sha256"],
        hex::encode(Sha256::digest(&payload))
    );
    assert!(aws_request.headers.get("x-amz-security-token").is_none());
    assert_ne!(aws_request.headers["x-amz-date"], "guest-forged");
    let azure_request = &received[1];
    assert_ne!(azure_request.headers["x-ms-date"], "guest-forged");
    // Independently verify Shared Key against the actual bytes/headers on the wire.
    use hmac::{Hmac, Mac};
    let mut ms_headers = azure_request
        .headers
        .iter()
        .filter(|(name, _)| name.as_str().starts_with("x-ms-"))
        .map(|(name, value)| (name.as_str(), value.to_str().unwrap().trim()))
        .collect::<Vec<_>>();
    ms_headers.sort_unstable();
    let canonical_headers = ms_headers
        .iter()
        .map(|(name, value)| format!("{name}:{value}\n"))
        .collect::<String>();
    let string_to_sign = format!(
        "PUT\n\n\n{}\n\napplication/octet-stream\n\n\n\n\n\n\n{canonical_headers}/fixture/fixture/container/file.bin",
        payload.len()
    );
    let mut mac = Hmac::<Sha256>::new_from_slice(b"synthetic-account-key").unwrap();
    mac.update(string_to_sign.as_bytes());
    let expected = format!(
        "SharedKey fixture:{}",
        STANDARD.encode(mac.finalize().into_bytes())
    );
    assert!(azure_request.headers["authorization"] == expected);
    provider.verify().await;
    Ok(())
}

#[tokio::test]
async fn native_public_egress_rejects_oversize_stream_and_does_not_follow_redirects()
-> anyhow::Result<()> {
    use wiremock::matchers::path;
    use wiremock::{Mock, MockServer, ResponseTemplate};
    allow_local_egress_for_this_test_binary();
    let provider = MockServer::start().await;
    Mock::given(path("/redirect"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", "/forbidden-follow"))
        .expect(1)
        .mount(&provider)
        .await;
    Mock::given(path("/forbidden-follow"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&provider)
        .await;
    Mock::given(path("/oversize"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0; 2048]))
        .expect(1)
        .mount(&provider)
        .await;
    let outbound = NativeOutboundHttp {
        facade: Arc::new(facade(test_pool().await, compatibility())),
        client: runtara_connections::net::build_hardened_client(),
    };
    let context = OutboundContext {
        tenant_id: "egress-fixture".into(),
        instance_id: None,
    };
    let redirect = outbound
        .request(
            &context,
            public_request(format!("{}/redirect", provider.uri()), 1_000),
        )
        .await
        .unwrap();
    assert_eq!(redirect.status, 302);
    let mut request = public_request(format!("{}/oversize", provider.uri()), 1_000);
    request.max_response_bytes = Some(1024);
    let failure = outbound.request(&context, request).await.unwrap_err();
    assert_eq!(failure.code, "RESPONSE_TOO_LARGE");
    provider.verify().await;
    Ok(())
}

#[cfg(feature = "valkey-integration-tests")]
#[tokio::test]
async fn native_preflight_rate_limit_returns_retry_headers_without_provider_io()
-> anyhow::Result<()> {
    use redis::AsyncCommands;
    use wiremock::matchers::any;
    use wiremock::{Mock, MockServer, ResponseTemplate};
    allow_local_egress_for_this_test_binary();
    init_server_config();
    let config = runtara_server::valkey::ValkeyConfig::from_env()
        .expect("valkey-integration-tests requires VALKEY_HOST");
    let mut manager =
        redis::aio::ConnectionManager::new(redis::Client::open(config.connection_url())?).await?;
    let pool = test_pool().await;
    let provider = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&provider)
        .await;
    let tenant = format!("preflight-{}", Uuid::new_v4());
    let connection = synthetic_connection(
        &pool,
        &tenant,
        "http_bearer",
        json!({"base_url":provider.uri(),"token":"synthetic-preflight"}),
    )
    .await?;
    sqlx::query("UPDATE connection_data_entity SET rate_limit_config=$1 WHERE id=$2")
        .bind(json!({"requestsPerSecond":1,"burstSize":1,"retryOnLimit":true,"maxRetries":1,"maxWaitMs":1000}))
        .bind(&connection).execute(&pool).await?;
    let key = format!("rate_limit:{connection}");
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis() as u64;
    let _: () = manager
        .hset_multiple(&key, &[("tokens", 0u64), ("last_refill", now_ms + 60_000)])
        .await?;
    let _: () = manager.expire(&key, 120).await?;
    let outbound = NativeOutboundHttp {
        facade: Arc::new(facade_with_redis(
            pool,
            compatibility(),
            Some(manager.clone()),
        )),
        client: runtara_connections::net::build_hardened_client(),
    };
    let response = outbound
        .request(
            &OutboundContext {
                tenant_id: tenant,
                instance_id: None,
            },
            connection_request(&connection, "/resource"),
        )
        .await
        .unwrap();
    assert_eq!(response.status, 429);
    assert!(
        response
            .headers
            .iter()
            .any(|(name, value)| name == "retry-after-ms" && value.parse::<u64>().unwrap() > 0)
    );
    assert!(
        response
            .headers
            .iter()
            .any(|(name, _)| name == "retry-after")
    );
    let _: () = manager.del(key).await?;
    provider.verify().await;
    Ok(())
}
