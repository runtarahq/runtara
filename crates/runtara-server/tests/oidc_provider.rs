//! Integration tests for `OidcProvider` — end-to-end JWKS fetch + signature
//! verification + tenant enforcement, using `wiremock` as a stand-in IdP and a
//! freshly generated RSA keypair for signing.

use axum::http::HeaderMap;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use rsa::RsaPrivateKey;
use rsa::pkcs1::EncodeRsaPrivateKey;
use rsa::traits::PublicKeyParts;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use runtara_server::auth::{
    AuthProvider, JwtConfig, jwks::JwksCache, provider::AuthError, providers::OidcProvider,
};

struct TestIdp {
    server: MockServer,
    encoding_key: EncodingKey,
    kid: String,
    issuer: String,
}

impl TestIdp {
    async fn start() -> Self {
        let server = MockServer::start().await;
        let kid = "test-kid".to_string();

        let mut rng = rand::thread_rng();
        let private_key = RsaPrivateKey::new(&mut rng, 2048).expect("rsa keygen");
        let pem = private_key
            .to_pkcs1_pem(rsa::pkcs1::LineEnding::LF)
            .expect("pkcs1 pem")
            .to_string();
        let encoding_key = EncodingKey::from_rsa_pem(pem.as_bytes()).expect("encoding key");

        let public = private_key.to_public_key();
        let n = URL_SAFE_NO_PAD.encode(public.n().to_bytes_be());
        let e = URL_SAFE_NO_PAD.encode(public.e().to_bytes_be());

        let jwks = json!({
            "keys": [{
                "kty": "RSA",
                "use": "sig",
                "alg": "RS256",
                "kid": kid,
                "n": n,
                "e": e,
            }]
        });

        Mock::given(method("GET"))
            .and(path("/jwks.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&jwks))
            .mount(&server)
            .await;

        let issuer = format!("{}/", server.uri());
        Self {
            server,
            encoding_key,
            kid,
            issuer,
        }
    }

    fn jwks_uri(&self) -> String {
        format!("{}/jwks.json", self.server.uri())
    }

    fn sign(&self, claims: &serde_json::Value) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(self.kid.clone());
        encode(&header, claims, &self.encoding_key).expect("sign jwt")
    }

    fn sign_with_kid(&self, claims: &serde_json::Value, kid: &str) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(kid.to_string());
        encode(&header, claims, &self.encoding_key).expect("sign jwt")
    }
}

async fn build_provider(idp: &TestIdp, tenant_id: &str) -> OidcProvider {
    let jwks_cache = JwksCache::new(idp.jwks_uri()).await;
    OidcProvider::new(
        JwtConfig {
            jwks_uri: idp.jwks_uri(),
            issuer: idp.issuer.clone(),
            audience: None,
        },
        jwks_cache,
        tenant_id.to_string(),
    )
}

fn bearer(token: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("Authorization", format!("Bearer {token}").parse().unwrap());
    headers
}

#[tokio::test]
async fn accepts_valid_token_with_matching_tenant() {
    let idp = TestIdp::start().await;
    let provider = build_provider(&idp, "org_123").await;

    let token = idp.sign(&json!({
        "sub": "user-1",
        "org_id": "org_123",
        "iss": idp.issuer,
        "exp": chrono::Utc::now().timestamp() + 3600,
    }));

    let ctx = provider.authenticate(&bearer(&token)).await.unwrap();
    assert_eq!(ctx.org_id, "org_123");
    assert_eq!(ctx.user_id, "user-1");
}

#[tokio::test]
async fn rejects_token_with_mismatched_tenant() {
    let idp = TestIdp::start().await;
    let provider = build_provider(&idp, "org_expected").await;

    let token = idp.sign(&json!({
        "sub": "user-1",
        "org_id": "org_other",
        "iss": idp.issuer,
        "exp": chrono::Utc::now().timestamp() + 3600,
    }));

    match provider.authenticate(&bearer(&token)).await {
        Err(AuthError::TenantMismatch(id)) => assert_eq!(id, "org_other"),
        other => panic!("expected TenantMismatch, got {other:?}"),
    }
}

#[tokio::test]
async fn rejects_missing_authorization_header() {
    let idp = TestIdp::start().await;
    let provider = build_provider(&idp, "org_123").await;

    match provider.authenticate(&HeaderMap::new()).await {
        Err(AuthError::MissingToken) => {}
        other => panic!("expected MissingToken, got {other:?}"),
    }
}

#[tokio::test]
async fn rejects_empty_bearer_token() {
    let idp = TestIdp::start().await;
    let provider = build_provider(&idp, "org_123").await;

    let mut headers = HeaderMap::new();
    headers.insert("Authorization", "Bearer ".parse().unwrap());
    match provider.authenticate(&headers).await {
        Err(AuthError::EmptyToken) => {}
        other => panic!("expected EmptyToken, got {other:?}"),
    }
}

#[tokio::test]
async fn rejects_token_signed_with_unknown_kid() {
    let idp = TestIdp::start().await;
    let provider = build_provider(&idp, "org_123").await;

    let token = idp.sign_with_kid(
        &json!({
            "sub": "user-1",
            "org_id": "org_123",
            "iss": idp.issuer,
            "exp": chrono::Utc::now().timestamp() + 3600,
        }),
        "unknown-kid",
    );

    match provider.authenticate(&bearer(&token)).await {
        Err(AuthError::InvalidToken) => {}
        other => panic!("expected InvalidToken, got {other:?}"),
    }
}

#[tokio::test]
async fn rejects_expired_token() {
    let idp = TestIdp::start().await;
    let provider = build_provider(&idp, "org_123").await;

    // jsonwebtoken allows 60s of clock skew by default; sign an obviously-expired
    // token to land safely past the leeway window.
    let token = idp.sign(&json!({
        "sub": "user-1",
        "org_id": "org_123",
        "iss": idp.issuer,
        "exp": chrono::Utc::now().timestamp() - 3600,
    }));

    match provider.authenticate(&bearer(&token)).await {
        Err(AuthError::InvalidToken) => {}
        other => panic!("expected InvalidToken for expired token, got {other:?}"),
    }
}
