//! The tokens of an authorization-code connection, sealed: seal sends the
//! refresh token to the token endpoint, the next pair comes back already
//! sealed, and this process never reads a token.
//!
//! Own binary: sets the loopback egress allowlist (read-once env).

use std::collections::HashMap;
use std::sync::Arc;

use runtara_connections::auth::provider_auth::{
    ResolvedConnectionAuth, evict_connection_token_caches, resolve_connection_auth,
};
use runtara_connections::sealed::{
    Allowances, MemoryKeySource, SealedCredentials, SendError, is_sealed,
};
use serde_json::{Value, json};
use wiremock::matchers::{body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TENANT: &str = "acme";
const TYPE: &str = "http_oauth2_authorization_code";

fn allow_loopback() {
    unsafe { std::env::set_var("RUNTARA_PROXY_ALLOWED_HOSTS", "127.0.0.1,localhost") };
}

fn sealed_for(server: &MockServer) -> SealedCredentials {
    let address = server.address();
    SealedCredentials::in_process(
        Arc::new(MemoryKeySource::default()),
        &[7_u8; 32],
        Allowances {
            private_hosts: vec![format!("{}:{}", address.ip(), address.port())],
            plain_http_hosts: vec![address.ip().to_string()],
            ..Default::default()
        },
    )
    .unwrap()
}

/// An authorized connection whose access token ran out an hour ago.
fn authorized(server: &MockServer, token_auth: &str) -> Value {
    json!({
        "auth_url": format!("{}/oauth/authorize", server.uri()),
        "token_url": format!("{}/oauth/token", server.uri()),
        "revocation_url": format!("{}/oauth/revoke", server.uri()),
        "base_url": format!("{}/api", server.uri()),
        "client_id": "cid",
        "client_secret": "csec",
        "token_auth": token_auth,
        "access_token": "at-0-CANARY",
        "refresh_token": "rt-0-CANARY",
        "token_expires_at": (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339(),
    })
}

async fn stored(sealed: &SealedCredentials, connection: &str, params: &Value) -> Value {
    let stored = sealed
        .seal_parameters(TENANT, connection, TYPE, params, None)
        .await
        .unwrap()
        .expect("the connection's tokens are sealed");
    for field in ["access_token", "refresh_token", "client_secret"] {
        assert!(is_sealed(&stored[field]));
    }
    assert!(!stored.to_string().contains("CANARY"));
    assert!(!stored.to_string().contains("csec"));
    stored
}

async fn resolve(
    sealed: &SealedCredentials,
    connection: &str,
    stored: &Value,
) -> Result<ResolvedConnectionAuth, String> {
    let mut headers = HashMap::new();
    let resolved = resolve_connection_auth(
        TENANT,
        connection,
        TYPE,
        stored,
        &mut headers,
        &Default::default(),
        sealed,
    )
    .await
    .map_err(|error| format!("{} (permanent: {})", error.message, error.permanent))?;
    assert!(headers.is_empty(), "{:?}", headers.keys());
    Ok(resolved)
}

/// A token endpoint that takes `refresh_token` and answers with the next pair.
async fn rotating(server: &MockServer, refresh_token: &str, next: u8) {
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .and(body_string_contains("grant_type=refresh_token"))
        .and(body_string_contains(
            format!("refresh_token={refresh_token}").as_str(),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": format!("at-{next}-CANARY"),
            "refresh_token": format!("rt-{next}-CANARY"),
            "expires_in": 3600,
        })))
        .expect(1)
        .mount(server)
        .await;
}

/// What the connection stores after a refresh: the pair it brought.
fn after(stored: &Value, resolved: &ResolvedConnectionAuth) -> Value {
    let rotated = resolved
        .rotated_sealed
        .as_ref()
        .expect("a refresh was made");
    let mut next = stored.clone();
    next["access_token"] = rotated.access_token.clone();
    next["refresh_token"] = rotated.refresh_token.clone().expect("the provider rotates");
    next["token_expires_at"] = json!(rotated.token_expires_at.unwrap().to_rfc3339());
    next
}

#[tokio::test]
async fn sealed_tokens_refresh_through_seal_and_stay_sealed() {
    allow_loopback();
    let server = MockServer::start().await;
    rotating(&server, "rt-0-CANARY", 1).await;
    rotating(&server, "rt-1-CANARY", 2).await;
    for token in [1, 2] {
        Mock::given(method("GET"))
            .and(path("/api/orders"))
            .and(header(
                "Authorization",
                format!("Bearer at-{token}-CANARY").as_str(),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_string("orders"))
            .expect(1)
            .mount(&server)
            .await;
    }
    let sealed = sealed_for(&server);
    let connection = "conn-sealed-rotation";
    let first = stored(&sealed, connection, &authorized(&server, "form_body")).await;

    let send = |parameters: Value| {
        let (sealed, url) = (&sealed, format!("{}/api/orders", server.uri()));
        async move {
            sealed
                .send_bounded(
                    TENANT,
                    connection,
                    TYPE,
                    &parameters,
                    "GET",
                    &url,
                    [],
                    None,
                    65536,
                )
                .await
        }
    };

    // The stored token ran out: seal refreshes, and the pair comes back sealed.
    let resolved = resolve(&sealed, connection, &first).await.unwrap();
    let presenting = resolved.sealed_parameters.clone().unwrap();
    assert!(!presenting.to_string().contains("CANARY"));
    assert_eq!(send(presenting).await.unwrap().status, 200);
    let second = after(&first, &resolved);
    assert!(!second.to_string().contains("CANARY"));
    assert_ne!(second["refresh_token"], first["refresh_token"]);

    // The client's credentials went as the provider takes them; seal added
    // the refresh token.
    let requests = server.received_requests().await.unwrap();
    let body = String::from_utf8_lossy(&requests[0].body).into_owned();
    assert!(
        body.contains("client_id=cid") && body.contains("client_secret=csec"),
        "{body}"
    );

    // The pair the connection now stores is the one its type calls for: an
    // edit that changes nothing about where tokens go keeps it.
    let kept = sealed
        .seal_parameters(TENANT, connection, TYPE, &second, Some(&second))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(kept, second);
    assert!(!sealed.holds_stale_tokens(TYPE, &second));

    // While the stored token is fresh there is no refresh, kept or not.
    evict_connection_token_caches(connection);
    let resolved = resolve(&sealed, connection, &second).await.unwrap();
    assert!(resolved.rotated_sealed.is_none());
    assert_eq!(
        resolved.sealed_parameters.unwrap()["access_token"],
        second["access_token"]
    );

    // It runs out again: the rotated refresh token gets the next pair.
    let mut expired = second.clone();
    expired["token_expires_at"] =
        json!((chrono::Utc::now() - chrono::Duration::minutes(1)).to_rfc3339());
    let resolved = resolve(&sealed, connection, &expired).await.unwrap();
    assert_eq!(
        send(resolved.sealed_parameters.clone().unwrap())
            .await
            .unwrap()
            .status,
        200
    );
    let third = after(&expired, &resolved);

    // A token goes to the API and nowhere else.
    let outside = sealed
        .send_bounded(
            TENANT,
            connection,
            TYPE,
            &third,
            "GET",
            &format!("{}/elsewhere", server.uri()),
            [],
            None,
            65536,
        )
        .await;
    assert!(matches!(outside, Err(SendError::Seal(_))), "{outside:?}");
}

#[tokio::test]
async fn a_client_that_authenticates_with_basic_refreshes_with_its_header() {
    allow_loopback();
    let server = MockServer::start().await;
    // "cid:csec", and the client's credentials are not in the body.
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .and(header("Authorization", "Basic Y2lkOmNzZWM="))
        .and(body_string_contains("refresh_token=rt-0-CANARY"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            // A provider that does not rotate sends no refresh token.
            "access_token": "at-1-CANARY", "expires_in": 3600,
        })))
        .expect(1)
        .mount(&server)
        .await;
    let sealed = sealed_for(&server);
    let connection = "conn-sealed-basic-refresh";
    let first = stored(&sealed, connection, &authorized(&server, "basic")).await;
    let resolved = resolve(&sealed, connection, &first).await.unwrap();
    let rotated = resolved.rotated_sealed.expect("a refresh was made");
    assert!(is_sealed(&rotated.access_token));
    // The refresh token held stays the connection's.
    assert!(rotated.refresh_token.is_none());
    let body =
        String::from_utf8_lossy(&server.received_requests().await.unwrap()[0].body).into_owned();
    assert!(!body.contains("csec"), "{body}");
}

#[tokio::test]
async fn a_refused_refresh_is_permanent_and_an_outage_is_not() {
    allow_loopback();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error": "invalid_grant", "error_description": "revoked",
        })))
        .mount(&server)
        .await;
    let sealed = sealed_for(&server);
    let first = stored(
        &sealed,
        "conn-sealed-dead",
        &authorized(&server, "form_body"),
    )
    .await;
    let refused = resolve(&sealed, "conn-sealed-dead", &first)
        .await
        .err()
        .expect("the provider refused");
    assert!(refused.contains("invalid_grant"), "{refused}");
    assert!(refused.contains("permanent: true"), "{refused}");
    assert!(!refused.contains("CANARY"));
}

/// Some providers (Zoho) refuse a grant with 200 and an OAuth `error`. It is
/// the refusal a 4xx is: permanent, with the code, and not a token that went
/// missing and is asked for again for ever.
#[tokio::test]
async fn a_grant_refused_with_200_is_permanent() {
    allow_loopback();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"error": "invalid_code"})))
        .mount(&server)
        .await;
    let sealed = sealed_for(&server);
    let connection = "conn-sealed-refused-200";
    let first = stored(&sealed, connection, &authorized(&server, "form_body")).await;
    let refused = resolve(&sealed, connection, &first)
        .await
        .err()
        .expect("the provider refused the refresh");
    assert!(refused.contains("invalid_code"), "{refused}");
    assert!(refused.contains("permanent: true"), "{refused}");

    let exchanged = sealed
        .exchange_code(
            TENANT,
            connection,
            TYPE,
            &first,
            "code-1",
            "https://platform.example.com/callback",
            None,
        )
        .await
        .err()
        .expect("the provider refused the code");
    assert!(exchanged.permanent, "{}", exchanged.message);
    assert!(
        exchanged.message.contains("invalid_code"),
        "{}",
        exchanged.message
    );
}

#[tokio::test]
async fn sealed_tokens_are_revoked_through_seal_and_go_stale_with_their_destination() {
    allow_loopback();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/revoke"))
        .and(wiremock::matchers::body_json(
            json!({"token": "rt-0-CANARY"}),
        ))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    let sealed = sealed_for(&server);
    let connection = "conn-sealed-revoke";
    let first = stored(&sealed, connection, &authorized(&server, "form_body")).await;
    assert_eq!(
        sealed.revoke_token(TENANT, connection, TYPE, &first).await,
        Ok(true)
    );
    // Nothing sealed, nothing to revoke here.
    assert_eq!(
        sealed
            .revoke_token(TENANT, connection, TYPE, &authorized(&server, "form_body"))
            .await,
        Ok(false)
    );

    // Pointed at another API, the tokens are for the wrong place.
    assert!(!sealed.holds_stale_tokens(TYPE, &first));
    let mut moved = first.clone();
    moved["base_url"] = json!("https://other.example.com");
    assert!(sealed.holds_stale_tokens(TYPE, &moved));
}

/// A connection on a platform OAuth app holds no client secret. The app's is
/// sealed for the refresh as the connection's own would be: seal sends it,
/// and the tokens come back sealed all the same.
#[tokio::test]
async fn a_platform_apps_client_secret_is_sealed_for_a_refresh() {
    allow_loopback();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .and(body_string_contains("refresh_token=rt-0-CANARY"))
        .and(body_string_contains("client_id=platform-client"))
        .and(body_string_contains("client_secret=platform-CANARY"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "at-1-CANARY", "expires_in": 3600,
        })))
        .expect(1)
        .mount(&server)
        .await;
    let sealed = sealed_for(&server);
    let connection = "conn-sealed-platform-client";
    let mut held = authorized(&server, "form_body");
    for field in ["client_id", "client_secret"] {
        held.as_object_mut().unwrap().remove(field);
    }
    held["oauth_app"] = json!("platform");
    let first = sealed
        .seal_parameters(TENANT, connection, TYPE, &held, None)
        .await
        .unwrap()
        .unwrap();
    assert!(is_sealed(&first["refresh_token"]));
    // Without a client, there is no refresh.
    assert!(resolve(&sealed, connection, &first).await.is_err());

    let effective = sealed
        .with_app_client(
            TENANT,
            connection,
            TYPE,
            &first,
            "platform-client",
            "platform-CANARY",
        )
        .await
        .unwrap();
    assert!(!effective.to_string().contains("CANARY"));
    let resolved = resolve(&sealed, connection, &effective).await.unwrap();
    assert!(is_sealed(
        &resolved
            .rotated_sealed
            .expect("a refresh was made")
            .access_token
    ));
}

/// With a sealed client secret, an authorization code is exchanged through
/// seal: the secret goes with the code, and the pair comes back sealed.
#[tokio::test]
async fn a_code_is_exchanged_through_seal_for_a_sealed_pair() {
    allow_loopback();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .and(body_string_contains("grant_type=authorization_code"))
        .and(body_string_contains("code=code-1"))
        .and(body_string_contains("code_verifier=verifier-1"))
        .and(body_string_contains("client_id=cid"))
        .and(body_string_contains("client_secret=csec"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "at-1-CANARY", "refresh_token": "rt-1-CANARY", "expires_in": 1800,
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/me"))
        .and(header("Authorization", "Bearer at-1-CANARY"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    let sealed = sealed_for(&server);
    let connection = "conn-sealed-exchange";
    // Not yet authorized: only the client's secret is held, sealed.
    let mut entered = authorized(&server, "form_body");
    for token in ["access_token", "refresh_token", "token_expires_at"] {
        entered.as_object_mut().unwrap().remove(token);
    }
    let held = sealed
        .seal_parameters(TENANT, connection, TYPE, &entered, None)
        .await
        .unwrap()
        .unwrap();
    assert!(is_sealed(&held["client_secret"]));

    let grant = sealed
        .exchange_code(
            TENANT,
            connection,
            TYPE,
            &held,
            "code-1",
            "https://platform.example.com/callback",
            Some("verifier-1"),
        )
        .await
        .map_err(|error| error.message)
        .unwrap();
    assert!(is_sealed(&grant.access_token));
    let refresh_token = grant
        .refresh_token
        .expect("the provider sent a refresh token");
    assert!(is_sealed(&refresh_token));
    assert_eq!(grant.expires_in, 1800);

    // The pair is the connection's to hold: stored, it is kept as the type
    // calls for, and its access token reaches the API.
    let mut authorized = held.clone();
    authorized["access_token"] = grant.access_token;
    authorized["refresh_token"] = refresh_token;
    assert!(!authorized.to_string().contains("CANARY"));
    assert!(!sealed.holds_stale_tokens(TYPE, &authorized));
    let kept = sealed
        .seal_parameters(TENANT, connection, TYPE, &authorized, Some(&authorized))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(kept, authorized);
    let response = sealed
        .send_bounded(
            TENANT,
            connection,
            TYPE,
            &authorized,
            "GET",
            &format!("{}/api/me", server.uri()),
            [],
            None,
            65536,
        )
        .await
        .unwrap();
    assert_eq!(response.status, 200);

    // A client secret seal does not hold is not sent: there is no exchange
    // but seal's.
    let refused = sealed
        .exchange_code(
            TENANT,
            connection,
            TYPE,
            &entered,
            "code-1",
            "https://x.example",
            None,
        )
        .await;
    assert!(refused.is_err());
}

/// A connection on a platform OAuth app holds no client secret. The app's is
/// sealed for the one exchange as the connection's own would be, so that
/// seal sends it too: the pair comes back sealed, and nothing readable is
/// ever the connection's to store, authorized or waiting to be confirmed.
#[tokio::test]
async fn a_platform_apps_code_is_exchanged_through_seal_too() {
    allow_loopback();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .and(body_string_contains("code=code-1"))
        .and(body_string_contains("client_id=platform-client"))
        .and(body_string_contains("client_secret=platform-CANARY"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "at-1-CANARY", "refresh_token": "rt-1-CANARY", "expires_in": 1800,
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/me"))
        .and(header("Authorization", "Bearer at-1-CANARY"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    let sealed = sealed_for(&server);
    let connection = "conn-platform-exchange";
    // As stored: the app's name, and no client of its own.
    let mut stored = authorized(&server, "form_body");
    for field in [
        "client_id",
        "client_secret",
        "access_token",
        "refresh_token",
        "token_expires_at",
    ] {
        stored.as_object_mut().unwrap().remove(field);
    }
    stored["oauth_app"] = json!("platform");

    let exchanging = sealed
        .with_app_client(
            TENANT,
            connection,
            TYPE,
            &stored,
            "platform-client",
            "platform-CANARY",
        )
        .await
        .unwrap();
    assert!(is_sealed(&exchanging["client_secret"]));
    assert_eq!(exchanging["client_id"], "platform-client");
    assert!(!exchanging.to_string().contains("CANARY"));

    let grant = sealed
        .exchange_code(
            TENANT,
            connection,
            TYPE,
            &exchanging,
            "code-1",
            "https://platform.example.com/callback",
            None,
        )
        .await
        .map_err(|error| error.message)
        .unwrap();
    assert!(is_sealed(&grant.access_token));

    // The pair is the stored connection's, which still names the app and
    // holds no client: kept as it is, and its access token reaches the API.
    let mut authorized = stored.clone();
    authorized["access_token"] = grant.access_token;
    authorized["refresh_token"] = grant.refresh_token.expect("a refresh token");
    assert!(!authorized.to_string().contains("CANARY"));
    assert!(authorized.get("client_secret").is_none());
    let kept = sealed
        .seal_parameters(TENANT, connection, TYPE, &authorized, Some(&authorized))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(kept, authorized);
    let response = sealed
        .send_bounded(
            TENANT,
            connection,
            TYPE,
            &authorized,
            "GET",
            &format!("{}/api/me", server.uri()),
            [],
            None,
            65536,
        )
        .await
        .unwrap();
    assert_eq!(response.status, 200);
}
