//! A connection's secrets, sealed where they are entered, over the HTTP
//! routes a caller uses, against real Postgres and Valkey.
//!
//! - A caller asks for an offer with the request it will save, seals its
//!   secrets under it, and saves the same request with the envelope.
//! - A secret sent readable is refused, on create and on update, by the save
//!   and by the offer.
//! - The envelope is good for the request it was offered for: saved beside
//!   another destination it is `OFFER_STALE`, and nothing is stored.
//! - A new connection has the id its offer gave; one that exists is not
//!   taken.
//! - An update enters one secret and leaves the others as they are held.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use runtara_connections::sealed::testing::{enter, save_as_a_caller, seal_under, without_sealed};
use runtara_connections::sealed::{is_entered, is_sealed};
use serde_json::{Value, json};
use tower::ServiceExt;

mod oauth_support;
mod valkey_support;
use oauth_support::*;

const SECRET: &str = "s3cr3t-CANARY-token";

async fn send(api: &Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(body.map_or_else(Body::empty, |body| Body::from(body.to_string())))
        .unwrap();
    let response = api.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

/// A save as a caller makes it, its secrets sealed first.
async fn save(api: &Router, method: &str, uri: &str, body: Value) -> (StatusCode, Value) {
    let (status, answer) = save_as_a_caller(
        |method, uri, body| async move {
            let (status, answer) = send(api, &method, &uri, body).await;
            (status.as_u16(), answer)
        },
        method,
        uri,
        body,
    )
    .await;
    (StatusCode::from_u16(status).unwrap(), answer)
}

fn bearer(title: &str, base_url: &str) -> Value {
    json!({
        "title": title,
        "integrationId": "http_bearer",
        "connectionParameters": {"base_url": base_url, "token": SECRET}
    })
}

/// `request` with its secrets marked as entered, and the secrets.
fn entered(request: &Value) -> (Value, std::collections::BTreeMap<String, String>) {
    let (parameters, secrets) = enter(
        request["integrationId"].as_str().unwrap(),
        &request["connectionParameters"],
    );
    let mut request = request.clone();
    request["connectionParameters"] = parameters;
    (request, secrets)
}

#[tokio::test]
async fn a_caller_seals_a_new_connections_secrets_under_an_offer() {
    let fixture = Fixture::start().await;
    let api = api_for(&fixture, "acme");
    let (request, secrets) = entered(&bearer("Orders", "https://api.example.com"));
    assert!(is_entered(&request["connectionParameters"]["token"]));

    let (status, offered) = send(&api, "POST", "/connections/offer", Some(request.clone())).await;
    assert_eq!(status, StatusCode::OK, "{offered}");
    let id = offered["connectionId"].as_str().expect("an id").to_owned();
    assert_eq!(offered["offer"]["item"], id.as_str());
    assert_eq!(offered["offer"]["keyset"], "acme");
    assert_eq!(offered["fields"], json!({"token": "token"}));
    assert!(offered["notAfter"].as_u64().is_some());
    assert!(!offered.to_string().contains(SECRET));
    // Nothing is stored for an offer.
    let (status, _) = send(&api, "GET", &format!("/connections/{id}"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let fields = serde_json::from_value(offered["fields"].clone()).unwrap();
    let mut saved = request;
    saved["connectionId"] = json!(id);
    saved["sealedSecrets"] = seal_under(&offered["offer"], &fields, &secrets);
    assert!(!saved.to_string().contains(SECRET));
    let (status, created) = send(&api, "POST", "/connections", Some(saved.clone())).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["connectionId"], id.as_str());

    let (stored, _) = fixture.stored(&id).await;
    assert!(is_sealed(&stored["token"]), "{stored}");
    assert_eq!(stored["base_url"], "https://api.example.com");
    assert!(!without_sealed(&stored).to_string().contains(SECRET));
    assert!(!stored.to_string().contains("$entered"));

    // The same request again: the id is taken.
    saved["title"] = json!("Orders again");
    let (status, answer) = send(&api, "POST", "/connections", Some(saved)).await;
    assert_eq!(status, StatusCode::CONFLICT, "{answer}");
}

#[tokio::test]
async fn a_readable_secret_is_refused_by_the_save_and_by_the_offer() {
    let fixture = Fixture::start().await;
    let api = api_for(&fixture, "acme");
    let readable = bearer("Orders", "https://api.example.com");

    for uri in ["/connections", "/connections/offer"] {
        let (status, answer) = send(&api, "POST", uri, Some(readable.clone())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}: {answer}");
        let said = answer.to_string();
        assert!(said.contains("`token` must be sealed"), "{uri}: {said}");
        assert!(!said.contains(SECRET), "{uri}: {said}");
    }
    let (status, listed) = send(&api, "GET", "/connections", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed["count"], 0, "{listed}");

    // Nor on an update, in the patch's `write`.
    let (status, created) = save(&api, "POST", "/connections", readable).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["connectionId"].as_str().unwrap().to_owned();
    let (_, held) = send(&api, "GET", &format!("/connections/{id}"), None).await;
    let update = json!({
        "version": held["connection"]["editProjection"]["version"],
        "connectionParameterPatch": {"write": {"token": "another-CANARY"}}
    });
    for uri in [
        format!("/connections/{id}/offer"),
        format!("/connections/{id}"),
    ] {
        let method = if uri.ends_with("/offer") {
            "POST"
        } else {
            "PUT"
        };
        let (status, answer) = send(&api, method, &uri, Some(update.clone())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}: {answer}");
        assert!(!answer.to_string().contains("another-CANARY"), "{answer}");
    }
}

#[tokio::test]
async fn an_envelope_is_good_for_the_request_it_was_offered_for() {
    let fixture = Fixture::start().await;
    let api = api_for(&fixture, "acme");
    let (request, secrets) = entered(&bearer("Orders", "https://api.example.com"));
    let (_, offered) = send(&api, "POST", "/connections/offer", Some(request.clone())).await;
    let fields = serde_json::from_value(offered["fields"].clone()).unwrap();
    let envelope = seal_under(&offered["offer"], &fields, &secrets);
    let id = offered["connectionId"].clone();

    // Saved beside another destination, as a captured request might be.
    let mut moved = request.clone();
    moved["connectionParameters"]["base_url"] = json!("https://attacker.example.net");
    moved["connectionId"] = id.clone();
    moved["sealedSecrets"] = envelope.clone();
    let (status, answer) = send(&api, "POST", "/connections", Some(moved)).await;
    assert_eq!(status, StatusCode::CONFLICT, "{answer}");
    assert_eq!(answer["error"], "OFFER_STALE", "{answer}");

    // With an id the offer did not give: sealed for another connection.
    let mut elsewhere = request.clone();
    elsewhere["connectionId"] = json!("0b0e2c1a-58a1-4f0e-8f0b-1d4d6f1f3a55");
    elsewhere["sealedSecrets"] = envelope.clone();
    let (status, answer) = send(&api, "POST", "/connections", Some(elsewhere)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");

    // With no id, or no envelope.
    let mut no_id = request.clone();
    no_id["sealedSecrets"] = envelope.clone();
    let (status, answer) = send(&api, "POST", "/connections", Some(no_id)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    let mut no_envelope = request.clone();
    no_envelope["connectionId"] = id.clone();
    let (status, answer) = send(&api, "POST", "/connections", Some(no_envelope)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");

    // By another tenant.
    let other = api_for(&fixture, "globex");
    let mut theirs = request.clone();
    theirs["connectionId"] = id.clone();
    theirs["sealedSecrets"] = envelope.clone();
    let (status, answer) = send(&other, "POST", "/connections", Some(theirs)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");

    // Nothing was stored by any of them.
    for api in [&api, &other] {
        let (_, listed) = send(api, "GET", "/connections", None).await;
        assert_eq!(listed["count"], 0, "{listed}");
    }

    // As it was offered, it is saved.
    let mut saved = request;
    saved["connectionId"] = id;
    saved["sealedSecrets"] = envelope;
    let (status, answer) = send(&api, "POST", "/connections", Some(saved)).await;
    assert_eq!(status, StatusCode::CREATED, "{answer}");
}

#[tokio::test]
async fn an_update_enters_one_secret_and_leaves_the_rest_as_held() {
    let fixture = Fixture::start().await;
    let api = api_for(&fixture, "acme");
    let (status, created) = save(
        &api,
        "POST",
        "/connections",
        json!({
            "title": "Chat",
            "integrationId": "slack_bot",
            "connectionParameters": {"bot_token": "xoxb-first", "signing_secret": SECRET}
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["connectionId"].as_str().unwrap().to_owned();
    let uri = format!("/connections/{id}");
    let (before, _) = fixture.stored(&id).await;
    assert!(is_sealed(&before["bot_token"]) && is_sealed(&before["signing_secret"]));

    let (_, held) = send(&api, "GET", &uri, None).await;
    let version = held["connection"]["editProjection"]["version"].clone();
    let update = json!({
        "version": version,
        "connectionParameterPatch": {"write": {"bot_token": {"$entered": true}}}
    });
    let (status, offered) = send(&api, "POST", &format!("{uri}/offer"), Some(update.clone())).await;
    assert_eq!(status, StatusCode::OK, "{offered}");
    assert_eq!(offered["connectionId"], id.as_str());
    assert_eq!(offered["fields"], json!({"bot_token": "bot_token"}));

    let fields = serde_json::from_value(offered["fields"].clone()).unwrap();
    let mut saved = update.clone();
    saved["sealedSecrets"] = seal_under(
        &offered["offer"],
        &fields,
        &[("bot_token".to_owned(), "xoxb-second".to_owned())].into(),
    );
    let (status, answer) = send(&api, "PUT", &uri, Some(saved.clone())).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert!(!answer.to_string().contains("xoxb-second"));

    let (after, _) = fixture.stored(&id).await;
    assert_eq!(after["signing_secret"], before["signing_secret"]);
    assert!(is_sealed(&after["bot_token"]));
    assert_ne!(after["bot_token"], before["bot_token"]);
    assert!(!without_sealed(&after).to_string().contains("xoxb-second"));

    // The same save again: the connection changed since.
    let (status, answer) = send(&api, "PUT", &uri, Some(saved)).await;
    assert_eq!(status, StatusCode::CONFLICT, "{answer}");
    assert_ne!(answer["error"], "OFFER_STALE", "{answer}");

    // An update that enters no secret needs no offer and no envelope.
    let (_, held) = send(&api, "GET", &uri, None).await;
    let (status, answer) = send(
        &api,
        "PUT",
        &uri,
        Some(json!({
            "version": held["connection"]["editProjection"]["version"],
            "title": "Chat, renamed"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    let (kept, _) = fixture.stored(&id).await;
    assert_eq!(kept["bot_token"], after["bot_token"]);
}

#[tokio::test]
async fn an_offer_is_asked_before_the_secrets_are_sealed() {
    let fixture = Fixture::start().await;
    let api = api_for(&fixture, "acme");
    let (mut request, _) = entered(&bearer("Orders", "https://api.example.com"));

    // A request that enters no secret gets an id and no offer.
    let (status, offered) = send(
        &api,
        "POST",
        "/connections/offer",
        Some(json!({
            "title": "Open",
            "integrationId": "mcp",
            "connectionParameters": {"url": "https://mcp.example.com/mcp", "auth_mode": "none"}
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{offered}");
    assert!(offered["offer"].is_null(), "{offered}");

    // What the save would refuse, the offer refuses first.
    let mut nameless = request.clone();
    nameless["title"] = json!("");
    let (status, _) = send(&api, "POST", "/connections/offer", Some(nameless)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let mut plain = request.clone();
    plain["connectionParameters"]["base_url"] = json!("ftp://api.example.com");
    let (status, _) = send(&api, "POST", "/connections/offer", Some(plain)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // An offer is not asked with an envelope already made.
    request["sealedSecrets"] = json!({});
    let (status, _) = send(&api, "POST", "/connections/offer", Some(request)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Nor for a connection that is not there.
    let (status, _) = send(
        &api,
        "POST",
        "/connections/0b0e2c1a-58a1-4f0e-8f0b-1d4d6f1f3a55/offer",
        Some(json!({"version": "1", "connectionParameterPatch": {"write": {"token": {"$entered": true}}}})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_database_password_is_sealed_and_the_rest_of_its_url_is_checked() {
    let fixture = Fixture::start().await;
    let api = api_for(&fixture, "acme");
    let database = |title: &str, url: &str| {
        json!({
            "title": title,
            "integrationId": "postgres",
            "connectionParameters": {"database_url": url}
        })
    };

    // The password is sealed; the URL without it says where it may go, and
    // is stored as the engine stores one: with its port, and bare for a
    // server that is verified.
    let (status, created) = save(
        &api,
        "POST",
        "/connections",
        database(
            "Orders",
            &format!("postgresql://app:{SECRET}@db.example.com/orders?sslmode=verify-full"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let (stored, _) = fixture
        .stored(created["connectionId"].as_str().unwrap())
        .await;
    assert!(is_sealed(&stored["database_url"]), "{stored}");
    assert_eq!(
        stored["database_target"],
        "postgres://app@db.example.com:5432/orders"
    );
    assert!(!without_sealed(&stored).to_string().contains(SECRET));

    // A URL that does not say how its session is protected is not stored.
    let (status, answer) = save(
        &api,
        "POST",
        "/connections",
        database(
            "Unsaid",
            &format!("postgres://app:{SECRET}@db.example.com/orders"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert!(answer.to_string().contains("sslmode"), "{answer}");

    // Nor is a password taken beside the secret, readable.
    let (status, answer) = send(
        &api,
        "POST",
        "/connections/offer",
        Some(json!({
            "title": "Readable",
            "integrationId": "postgres",
            "connectionParameters": {
                "database_url": {"$entered": true},
                "database_target": format!("postgres://app:{SECRET}@db.example.com/orders?sslmode=require")
            }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert!(!answer.to_string().contains(SECRET), "{answer}");
}
