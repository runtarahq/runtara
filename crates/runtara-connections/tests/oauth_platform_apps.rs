//! Connections on OAuth apps the platform registered, against real Postgres
//! and Valkey.
//!
//! - A connection names the app in `oauth_app` and has no client credentials
//!   of its own; the app's are added only to what goes to the provider.
//! - The connection-type listing offers the apps a tenant may use.
//! - A connection asks for the app's scopes or a subset, never more.
//! - The generic type, another type's app, an app the tenant may not use and
//!   own credentials alongside an app are refused.
//! - Moving between a platform app and the tenant's own is a change of
//!   client: the connection must be authorized again.

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use runtara_connections::config::{EveryTenant, OAuthOptions};
use runtara_connections::oauth_apps::{OAuthAppCredentials, OAuthAppSummary, OAuthApps};
use runtara_connections::service::oauth::StartBinding;
use runtara_connections::{Caller, CallerChannel, TenantId, connections_router};
use serde_json::{Value, json};
use tower::ServiceExt;
use zeroize::Zeroizing;

mod oauth_support;
mod valkey_support;
use oauth_support::*;

/// A HubSpot app every tenant but `beta` may use.
struct PlatformApps(Arc<OAuthAppCredentials>);

impl PlatformApps {
    fn new() -> Self {
        Self(Arc::new(OAuthAppCredentials {
            app_id: "hubspot".into(),
            integration_id: "hubspot_private_app".into(),
            display_name: "HubSpot".into(),
            client_id: "platform-client".into(),
            client_secret: Zeroizing::new("platform-secret".into()),
            scopes: "oauth crm.objects.contacts.read crm.objects.deals.read".into(),
            required_scopes: "oauth".into(),
        }))
    }
}

impl OAuthApps for PlatformApps {
    fn get(&self, tenant: &str, app_id: &str) -> Option<Arc<OAuthAppCredentials>> {
        (tenant != "beta" && app_id == self.0.app_id).then(|| self.0.clone())
    }
    fn available(&self, tenant: &str) -> Vec<OAuthAppSummary> {
        if tenant == "beta" {
            Vec::new()
        } else {
            vec![self.0.as_ref().into()]
        }
    }
}

fn oauth(tenantless_callback: bool) -> OAuthOptions {
    OAuthOptions {
        tenantless_callback,
        apps: Some(Arc::new(PlatformApps::new())),
        ..OAuthOptions::default()
    }
}

fn api(fixture: &Fixture, tenant: &'static str, oauth: OAuthOptions) -> Router {
    let mut config = fixture.config(Arc::new(EveryTenant));
    config.oauth = oauth;
    connections_router(config).layer(axum::middleware::from_fn(
        move |mut request: axum::extract::Request, next: axum::middleware::Next| async move {
            request
                .extensions_mut()
                .insert(TenantId(tenant.to_string()));
            request.extensions_mut().insert(Caller {
                id: "user-1".into(),
                channel: CallerChannel::Member,
            });
            next.run(request).await
        },
    ))
}

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

fn hubspot(params: Value) -> Value {
    json!({
        "title": format!("HubSpot {}", uuid::Uuid::new_v4()),
        "integrationId": "hubspot_private_app",
        "connectionParameters": params,
    })
}

async fn create(api: &Router, body: Value) -> (StatusCode, Value) {
    send(api, "POST", "/connections", Some(body)).await
}

fn created_id(body: &Value) -> String {
    body["connectionId"]
        .as_str()
        .or(body["id"].as_str())
        .expect("created id")
        .to_string()
}

#[tokio::test]
async fn a_connection_on_a_platform_app_has_no_credentials_of_its_own() {
    let fixture = Fixture::start().await;
    let api = api(&fixture, "acme", oauth(true));

    let (status, body) = create(
        &api,
        hubspot(json!({ "oauth_app": "hubspot", "scopes": "crm.objects.deals.read" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let id = created_id(&body);
    let (stored, status) = fixture.stored(&id).await;
    assert_eq!(status, "REQUIRES_RECONNECTION");
    assert_eq!(stored["oauth_app"], "hubspot");
    assert_eq!(stored["oauth_callback"], "tenantless");
    assert!(stored.get("client_id").is_none(), "{stored}");
    assert!(stored.get("client_secret").is_none(), "{stored}");

    // The console names the app in place of the credentials.
    let (_, got) = send(&api, "GET", &format!("/connections/{id}"), None).await;
    assert_eq!(
        got["connection"]["editProjection"]["values"]["oauth_app"], "hubspot",
        "{got}"
    );
    assert!(!got.to_string().contains("platform-secret"));

    // The provider is sent the app's client and the connection's scopes,
    // with the app's required scope, to the shared callback.
    let service = fixture
        .service()
        .with_apps(Some(Arc::new(PlatformApps::new())));
    let flow = service
        .start(&id, "acme", StartBinding::Browser)
        .await
        .expect("start");
    let url = flow.authorization_url;
    assert!(
        url.starts_with("https://app.hubspot.com/oauth/authorize?"),
        "the flow goes to HubSpot's authorize endpoint"
    );
    assert_eq!(
        query_param(&url, "client_id").as_deref(),
        Some("platform-client")
    );
    assert_eq!(
        query_param(&url, "scope").as_deref(),
        Some("crm.objects.deals.read oauth")
    );
    assert_eq!(
        query_param(&url, "redirect_uri").as_deref(),
        Some(format!("{BASE}/api/oauth/callback").as_str())
    );
    assert!(!url.contains("platform-secret"));

    // Without the app resolvable (another tenant, or no apps), nothing starts.
    assert!(
        fixture
            .service()
            .start(&id, "acme", StartBinding::Browser)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn the_listing_offers_the_apps_a_tenant_may_use() {
    let fixture = Fixture::start().await;
    let types = |tenant: &'static str, oauth: OAuthOptions| {
        let api = api(&fixture, tenant, oauth);
        async move { send(&api, "GET", "/connections/types", None).await.1 }
    };
    let managed = |listing: &Value, integration: &str| {
        listing["connectionTypes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["integrationId"] == integration)
            .unwrap()
            .get("managedApps")
            .cloned()
    };

    let acme = types("acme", oauth(true)).await;
    let apps = managed(&acme, "hubspot_private_app").expect("HubSpot offers the app");
    assert_eq!(apps[0]["id"], "hubspot");
    assert_eq!(apps[0]["requiredScopes"], "oauth");
    assert!(!acme.to_string().contains("platform-secret"));
    assert!(!acme.to_string().contains("platform-client"));
    assert!(managed(&acme, "quickbooks_online").is_none());
    assert!(managed(&types("beta", oauth(true)).await, "hubspot_private_app").is_none());
    // Without the shared callback, platform apps cannot be used.
    assert!(managed(&types("acme", oauth(false)).await, "hubspot_private_app").is_none());

    let (status, one) = send(
        &api(&fixture, "acme", oauth(true)),
        "GET",
        "/connections/types/hubspot_private_app",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(one["connectionType"]["managedApps"][0]["id"], "hubspot");
}

#[tokio::test]
async fn a_platform_app_is_refused_where_it_does_not_fit() {
    let fixture = Fixture::start().await;
    let acme = api(&fixture, "acme", oauth(true));
    let refused = [
        (
            "the generic type",
            json!({
                "title": "Generic",
                "integrationId": "http_oauth2_authorization_code",
                "connectionParameters": {
                    "oauth_app": "hubspot",
                    "auth_url": "https://auth.example.com/authorize",
                    "token_url": "https://auth.example.com/token",
                    "base_url": "https://api.example.com"
                }
            }),
        ),
        (
            "another type",
            json!({
                "title": "QuickBooks",
                "integrationId": "quickbooks_online",
                "connectionParameters": { "oauth_app": "hubspot" }
            }),
        ),
        (
            "an unknown app",
            hubspot(json!({ "oauth_app": "salesforce" })),
        ),
        (
            "own credentials alongside",
            hubspot(
                json!({ "oauth_app": "hubspot", "client_id": "mine", "client_secret": "mine" }),
            ),
        ),
        (
            "a scope the app lacks",
            hubspot(json!({ "oauth_app": "hubspot", "scopes": "crm.objects.contacts.write" })),
        ),
    ];
    for (case, body) in refused {
        let (status, body) = create(&acme, body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{case}: {body}");
    }
    let (status, body) = create(
        &api(&fixture, "beta", oauth(true)),
        hubspot(json!({ "oauth_app": "hubspot" })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a tenant without the app: {body}"
    );
    let (status, body) = create(
        &api(&fixture, "acme", oauth(false)),
        hubspot(json!({ "oauth_app": "hubspot" })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "no shared callback: {body}"
    );
    // A form submitting the type's default scopes asks for the app's set.
    let (status, body) = create(
        &acme,
        hubspot(json!({
            "oauth_app": "hubspot",
            "scopes": "oauth crm.objects.contacts.read crm.objects.contacts.write crm.objects.companies.read crm.objects.companies.write crm.objects.deals.read crm.objects.deals.write crm.objects.quotes.read crm.objects.quotes.write crm.objects.line_items.read crm.objects.line_items.write crm.objects.owners.read"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
}

#[tokio::test]
async fn moving_between_a_platform_app_and_the_tenants_own_needs_reauthorization() {
    let fixture = Fixture::start().await;
    let api = api(&fixture, "acme", oauth(true));
    let (status, body) = create(
        &api,
        hubspot(json!({ "client_id": "own-client", "client_secret": "own-secret" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let id = created_id(&body);
    sqlx::query(
        "UPDATE connection_data_entity SET status = 'ACTIVE', connection_parameters = \
         connection_parameters || '{\"access_token\":\"at-own\",\"refresh_token\":\"rt-own\"}' \
         WHERE id = $1",
    )
    .bind(&id)
    .execute(&fixture.pool)
    .await
    .unwrap();

    let version = |api: Router, id: String| async move {
        let (_, got) = send(&api, "GET", &format!("/connections/{id}"), None).await;
        got["connection"]["editProjection"]["version"]
            .as_str()
            .or(got["connection"]["updatedAt"].as_str())
            .expect("version")
            .to_string()
    };
    let patch = |version: String, patch: Value| json!({ "version": version, "connectionParameterPatch": patch });

    // Onto the platform app: the own credentials and the tokens go.
    let (status, body) = send(
        &api,
        "PUT",
        &format!("/connections/{id}"),
        Some(patch(
            version(api.clone(), id.clone()).await,
            json!({ "set": { "oauth_app": "hubspot" } }),
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (stored, status) = fixture.stored(&id).await;
    assert_eq!(status, "REQUIRES_RECONNECTION");
    assert_eq!(stored["oauth_app"], "hubspot");
    for gone in [
        "client_id",
        "client_secret",
        "access_token",
        "refresh_token",
    ] {
        assert!(stored.get(gone).is_none(), "{gone}: {stored}");
    }

    // A secret cannot be written onto a platform app's connection.
    let (status, _) = send(
        &api,
        "PUT",
        &format!("/connections/{id}"),
        Some(patch(
            version(api.clone(), id.clone()).await,
            json!({ "write": { "client_secret": "sneaky" } }),
        )),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Off it, only with the tenant's own credentials.
    let (status, _) = send(
        &api,
        "PUT",
        &format!("/connections/{id}"),
        Some(patch(
            version(api.clone(), id.clone()).await,
            json!({ "clear": ["oauth_app"] }),
        )),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, body) = send(
        &api,
        "PUT",
        &format!("/connections/{id}"),
        Some(patch(
            version(api.clone(), id.clone()).await,
            json!({
                "clear": ["oauth_app"],
                "set": { "client_id": "own-client-2" },
                "write": { "client_secret": "own-secret-2" }
            }),
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (stored, status) = fixture.stored(&id).await;
    assert_eq!(status, "REQUIRES_RECONNECTION");
    assert!(stored.get("oauth_app").is_none());
    assert_eq!(stored["client_id"], "own-client-2");
    assert_eq!(stored["client_secret"], "own-secret-2");
}

#[tokio::test]
async fn requests_use_the_app_while_the_tenant_may_and_fail_when_it_may_not() {
    let fixture = Fixture::start().await;
    let stored = json!({
        "oauth_app": "hubspot",
        "access_token": "at-live",
        "token_expires_at": (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339(),
    });
    let facade = |oauth: OAuthOptions| {
        let mut config = fixture.config(Arc::new(EveryTenant));
        config.oauth = oauth;
        runtara_connections::ConnectionsFacade::new(
            runtara_connections::ConnectionsState::from_config(config),
        )
    };

    let mut headers = std::collections::HashMap::new();
    facade(oauth(true))
        .resolve_connection_auth("c1", "acme", "hubspot_private_app", &stored, &mut headers)
        .await
        .expect("the tenant may use the app");
    assert_eq!(
        headers.get("Authorization").map(String::as_str),
        Some("Bearer at-live")
    );

    // A tenant that lost the app gets a clear failure, and the connection
    // is kept.
    let mut headers = std::collections::HashMap::new();
    let Err(error) = facade(oauth(true))
        .resolve_connection_auth("c1", "beta", "hubspot_private_app", &stored, &mut headers)
        .await
    else {
        panic!("beta may not use the app");
    };
    assert!(error.to_string().contains("hubspot"), "{error}");
    assert!(headers.is_empty());
}
