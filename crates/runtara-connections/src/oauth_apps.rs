//! OAuth apps the platform registered with a provider once, for tenants to
//! connect with instead of registering their own.
//!
//! A connection names one in its `oauth_app` parameter in place of a client
//! ID and secret. The app's secret is sealed as it is entered, to the
//! platform's own key, for seal to send with any tenant's connection on the
//! app and only to the app's token endpoint
//! ([`crate::sealed::SealedCredentials::seal_app_secret`]). Nothing holds it
//! readable: not a file, not the database, not this process. It is resolved
//! where a request to the provider needs it, and is never written into the
//! connection, so nothing that stores the connection back can persist it.
//!
//! The host supplies the apps ([`OAuthApps`]); runtara keeps them in its
//! database ([`StoredOAuthApps`], written by [`put`]) when it supplies none.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, LazyLock, RwLock};

use opentelemetry::KeyValue;
use opentelemetry::metrics::Counter;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;

use crate::sealed::SealedCredentials;

/// The reserved connection parameter naming the platform app a connection
/// uses.
pub const OAUTH_APP_PARAM: &str = "oauth_app";

/// A platform app, with its client secret sealed.
pub struct OAuthAppCredentials {
    pub app_id: String,
    /// The one connection type it may serve.
    pub integration_id: String,
    pub display_name: String,
    pub client_id: String,
    /// The client secret, sealed: the marker
    /// [`crate::sealed::SealedCredentials::seal_app_secret`] made. Nothing
    /// here can read it.
    pub client_secret: Value,
    /// The parameters of the app's connections that say where its client
    /// secret goes, for a type with several token endpoints (a Zoho
    /// `data_center`): an object. The secret is sealed for those, and a
    /// connection elsewhere cannot use the app.
    pub parameters: Value,
    /// The scopes the app was registered for, space-separated.
    pub scopes: String,
    /// The scopes always requested, a subset of `scopes`.
    pub required_scopes: String,
}

impl std::fmt::Debug for OAuthAppCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthAppCredentials")
            .field("app_id", &self.app_id)
            .field("integration_id", &self.integration_id)
            .field("client_id", &self.client_id)
            .finish_non_exhaustive()
    }
}

/// A platform app as a tenant's connection form shows it. No secrets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct OAuthAppSummary {
    pub id: String,
    pub integration_id: String,
    pub display_name: String,
    pub scopes: String,
    pub required_scopes: String,
}

impl From<&OAuthAppCredentials> for OAuthAppSummary {
    fn from(app: &OAuthAppCredentials) -> Self {
        Self {
            id: app.app_id.clone(),
            integration_id: app.integration_id.clone(),
            display_name: app.display_name.clone(),
            scopes: app.scopes.clone(),
            required_scopes: app.required_scopes.clone(),
        }
    }
}

/// The platform apps each tenant may use. Answers from memory: it is asked
/// on the request path.
pub trait OAuthApps: Send + Sync {
    /// `app_id`'s credentials if `tenant` may use it; `None` otherwise.
    fn get(&self, tenant: &str, app_id: &str) -> Option<Arc<OAuthAppCredentials>>;
    /// The apps `tenant` may use.
    fn available(&self, tenant: &str) -> Vec<OAuthAppSummary>;
}

/// Errors providers answered to requests made with a platform app, by app,
/// stage (`exchange`, `refresh`) and kind: every tenant on an app shares its
/// standing with the provider, so a rise in `rate_limited` or
/// `invalid_client` on one app is worth an alert.
static PROVIDER_ERRORS: LazyLock<Counter<u64>> = LazyLock::new(|| {
    opentelemetry::global::meter("runtara-connections")
        .u64_counter("runtara.oauth_app.provider_errors")
        .with_description(
            "Errors a provider answered to requests made with a platform OAuth app, by app, stage and kind",
        )
        .build()
});

/// The kind of a token endpoint's error, from its message (`... returned
/// {status}: {error} ...`): what an alert on a platform app watches for.
pub fn provider_error_kind(message: &str) -> &'static str {
    let status = message
        .split_once("returned ")
        .and_then(|(_, rest)| rest.get(..3))
        .and_then(|status| status.parse::<u16>().ok());
    if status == Some(429) {
        "rate_limited"
    } else if message.contains("invalid_client") {
        "invalid_client"
    } else if message.contains("invalid_grant") {
        "invalid_grant"
    } else if status.is_some_and(|status| status >= 500) {
        "provider_unavailable"
    } else {
        "other"
    }
}

/// Counts an error a provider answered to a request `app_id` made at `stage`.
pub fn record_provider_error(app_id: &str, stage: &'static str, message: &str) {
    PROVIDER_ERRORS.add(
        1,
        &[
            KeyValue::new("app", app_id.to_owned()),
            KeyValue::new("stage", stage),
            KeyValue::new("kind", provider_error_kind(message)),
        ],
    );
}

/// A platform app as it is entered, without its secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewOAuthApp {
    pub id: String,
    pub integration_id: String,
    pub display_name: String,
    pub client_id: String,
    pub scopes: String,
    pub required_scopes: String,
    /// See [`OAuthAppCredentials::parameters`].
    pub parameters: Value,
}

/// Seals `client_secret` for `app` and stores the app, in place of any with
/// its id. The secret is not kept anywhere else, and cannot be read back.
pub async fn put(
    pool: &PgPool,
    sealed: &SealedCredentials,
    app: &NewOAuthApp,
    client_secret: &str,
) -> Result<(), String> {
    check_app(app)?;
    if client_secret.is_empty() {
        return Err(format!("OAuth app '{}' needs a client secret", app.id));
    }
    let marker = sealed
        .seal_app_secret(
            &app.id,
            &app.integration_id,
            &app.client_id,
            &app.parameters,
            client_secret,
        )
        .await
        .map_err(|error| format!("OAuth app '{}': {error}", app.id))?;
    sqlx::query(
        r#"
        INSERT INTO platform_oauth_app
            (id, integration_id, display_name, client_id, client_secret, scopes,
             required_scopes, parameters)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        ON CONFLICT (id) DO UPDATE SET
            integration_id = EXCLUDED.integration_id,
            display_name = EXCLUDED.display_name,
            client_id = EXCLUDED.client_id,
            client_secret = EXCLUDED.client_secret,
            scopes = EXCLUDED.scopes,
            required_scopes = EXCLUDED.required_scopes,
            parameters = EXCLUDED.parameters,
            updated_at = NOW()
        "#,
    )
    .bind(&app.id)
    .bind(&app.integration_id)
    .bind(&app.display_name)
    .bind(&app.client_id)
    .bind(&marker)
    .bind(&app.scopes)
    .bind(&app.required_scopes)
    .bind(&app.parameters)
    .execute(pool)
    .await
    .map_err(|error| format!("OAuth app '{}' was not stored: {error}", app.id))?;
    Ok(())
}

/// Removes a stored app. `false` if there was none. Connections on it stop
/// working until it is entered again.
pub async fn remove(pool: &PgPool, app_id: &str) -> Result<bool, String> {
    sqlx::query("DELETE FROM platform_oauth_app WHERE id = $1")
        .bind(app_id)
        .execute(pool)
        .await
        .map(|deleted| deleted.rows_affected() > 0)
        .map_err(|error| format!("OAuth app '{app_id}' was not removed: {error}"))
}

/// The stored apps as they were entered, by id. No secrets.
pub async fn entered(pool: &PgPool) -> Result<Vec<NewOAuthApp>, String> {
    Ok(stored(pool)
        .await?
        .into_iter()
        .map(|app| NewOAuthApp {
            id: app.app_id,
            integration_id: app.integration_id,
            display_name: app.display_name,
            client_id: app.client_id,
            scopes: app.scopes,
            required_scopes: app.required_scopes,
            parameters: app.parameters,
        })
        .collect())
}

/// The stored apps, by id.
async fn stored(pool: &PgPool) -> Result<Vec<OAuthAppCredentials>, String> {
    type Row = (String, String, String, String, Value, String, String, Value);
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT id, integration_id, display_name, client_id, client_secret, scopes, \
                required_scopes, parameters \
         FROM platform_oauth_app ORDER BY id",
    )
    .fetch_all(pool)
    .await
    .map_err(|error| format!("the platform OAuth apps could not be read: {error}"))?;
    Ok(rows
        .into_iter()
        .map(
            |(
                app_id,
                integration_id,
                display_name,
                client_id,
                client_secret,
                scopes,
                required_scopes,
                parameters,
            )| {
                OAuthAppCredentials {
                    app_id,
                    integration_id,
                    display_name,
                    client_id,
                    client_secret,
                    parameters,
                    scopes,
                    required_scopes,
                }
            },
        )
        .collect())
}

/// The platform apps in runtara's database, every one available to every
/// tenant. It answers from memory, and reads the database when told to
/// ([`Self::reload`]).
pub struct StoredOAuthApps {
    pool: PgPool,
    apps: RwLock<HashMap<String, Arc<OAuthAppCredentials>>>,
}

impl StoredOAuthApps {
    /// The stored apps, read now.
    pub async fn load(pool: PgPool, sealed: &SealedCredentials) -> Result<Self, String> {
        let apps = Self {
            pool,
            apps: RwLock::new(HashMap::new()),
        };
        apps.reload(sealed).await?;
        Ok(apps)
    }

    /// Reads the stored apps again, so that one entered, changed or removed
    /// since is seen. An app whose secret this deployment cannot open (its
    /// row came from another deployment, or the platform's seal key is gone)
    /// is left out, and named in the log.
    pub async fn reload(&self, sealed: &SealedCredentials) -> Result<(), String> {
        let mut apps = HashMap::new();
        for app in stored(&self.pool).await? {
            if !sealed
                .holds_app_secret(&app.app_id, &app.client_secret)
                .await
            {
                tracing::error!(
                    app = %app.app_id,
                    "A platform OAuth app's sealed client secret cannot be opened here; \
                     enter the app again"
                );
                continue;
            }
            apps.insert(app.app_id.clone(), Arc::new(app));
        }
        *self.apps.write().unwrap_or_else(|e| e.into_inner()) = apps;
        Ok(())
    }
}

impl OAuthApps for StoredOAuthApps {
    fn get(&self, _tenant: &str, app_id: &str) -> Option<Arc<OAuthAppCredentials>> {
        let apps = self.apps.read().unwrap_or_else(|e| e.into_inner());
        apps.get(app_id).cloned()
    }

    fn available(&self, _tenant: &str) -> Vec<OAuthAppSummary> {
        let apps = self.apps.read().unwrap_or_else(|e| e.into_inner());
        let mut apps: Vec<_> = apps.values().map(|app| app.as_ref().into()).collect();
        apps.sort_by(|a: &OAuthAppSummary, b| a.id.cmp(&b.id));
        apps
    }
}

/// Whether `app` can serve a connection at all.
pub fn check_app(app: &NewOAuthApp) -> Result<(), String> {
    let id = &app.id;
    if id.is_empty() || app.client_id.is_empty() {
        return Err(format!("OAuth app '{id}' needs an id and a client ID"));
    }
    if !app.parameters.is_object() {
        return Err(format!(
            "OAuth app '{id}': its parameters are not an object"
        ));
    }
    let Some(oauth_config) = runtara_agents::registry::find_connection_type(&app.integration_id)
        .and_then(|meta| meta.oauth_config)
    else {
        return Err(format!(
            "OAuth app '{id}': '{}' is not an authorization-code connection type",
            app.integration_id
        ));
    };
    if oauth_config.params_driven {
        return Err(format!(
            "OAuth app '{id}': '{}' takes its endpoints from the connection, so it cannot use a \
             platform app",
            app.integration_id
        ));
    }
    let registered = scope_set(&app.scopes);
    if let Some(scope) = scope_set(&app.required_scopes)
        .into_iter()
        .find(|scope| !registered.contains(scope))
    {
        return Err(format!(
            "OAuth app '{id}' requires scope '{scope}', which it was not registered for"
        ));
    }
    Ok(())
}

fn scope_set(scopes: &str) -> BTreeSet<&str> {
    scopes.split_whitespace().collect()
}

/// The scopes a connection on `app` asks for: the app's whole set when
/// `requested` is empty or the connection type's default (a form submits that
/// unchanged), else `requested` with the required scopes added. Refuses a
/// scope the app was not registered for, naming it.
pub fn scopes_for(
    app: &OAuthAppCredentials,
    requested: Option<&str>,
    type_default: &str,
) -> Result<String, String> {
    let requested = requested.map(str::trim).unwrap_or_default();
    if requested.is_empty() || scope_set(requested) == scope_set(type_default) {
        return Ok(app.scopes.clone());
    }
    let registered = scope_set(&app.scopes);
    if let Some(scope) = scope_set(requested)
        .into_iter()
        .find(|scope| !registered.contains(scope))
    {
        return Err(format!(
            "scope '{scope}' is not one the platform app '{}' was registered for",
            app.app_id
        ));
    }
    let mut scopes: Vec<&str> = Vec::new();
    for scope in requested
        .split_whitespace()
        .chain(app.required_scopes.split_whitespace())
    {
        if !scopes.contains(&scope) {
            scopes.push(scope);
        }
    }
    Ok(scopes.join(" "))
}

/// The platform app a connection's `params` name, resolved for `tenant`.
/// `Ok(None)` when they name none; an error when they name one the tenant may
/// not use, or one for another connection type.
pub fn app_for(
    apps: Option<&dyn OAuthApps>,
    tenant: &str,
    integration_id: &str,
    params: &Value,
) -> Result<Option<Arc<OAuthAppCredentials>>, String> {
    let Some(app_id) = params.get(OAUTH_APP_PARAM).and_then(Value::as_str) else {
        return Ok(None);
    };
    // A type that takes its endpoints from the connection would send the
    // platform's secret wherever a tenant pointed it.
    if runtara_agents::registry::find_connection_type(integration_id)
        .and_then(|meta| meta.oauth_config)
        .is_none_or(|oauth_config| oauth_config.params_driven)
    {
        return Err(format!(
            "'{integration_id}' connections cannot use a platform OAuth app"
        ));
    }
    let app = apps
        .and_then(|apps| apps.get(tenant, app_id))
        .ok_or_else(|| format!("the platform OAuth app '{app_id}' is not available"))?;
    if app.integration_id != integration_id {
        return Err(format!(
            "the platform OAuth app '{app_id}' is for '{}', not '{integration_id}'",
            app.integration_id
        ));
    }
    Ok(Some(app))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A sealed value's marker, as a test that sends nothing needs one.
    pub(crate) fn app(id: &str, integration: &str) -> OAuthAppCredentials {
        OAuthAppCredentials {
            app_id: id.into(),
            integration_id: integration.into(),
            display_name: "HubSpot".into(),
            client_id: "platform-client".into(),
            client_secret: serde_json::json!({"$sealed": {"record": "oauth-app:test"}}),
            scopes: "oauth crm.objects.contacts.read crm.objects.deals.read".into(),
            required_scopes: "oauth".into(),
            parameters: serde_json::json!({}),
        }
    }

    #[test]
    fn an_app_is_checked_as_it_is_entered() {
        let entered = |integration: &str, required: &str, parameters: Value| NewOAuthApp {
            id: "app".into(),
            integration_id: integration.into(),
            display_name: "App".into(),
            client_id: "c".into(),
            scopes: "a".into(),
            required_scopes: required.into(),
            parameters,
        };
        let none = || serde_json::json!({});
        check_app(&entered("hubspot_private_app", "a", none())).unwrap();
        check_app(&entered(
            "zoho_crm_oauth",
            "",
            serde_json::json!({"data_center": "eu"}),
        ))
        .unwrap();
        assert!(!format!("{:?}", app("hubspot", "hubspot_private_app")).contains("client_secret"));
        for (bad, why) in [
            (
                entered("http_oauth2_authorization_code", "", none()),
                "takes its endpoints",
            ),
            (entered("stripe", "", none()), "not an authorization-code"),
            (
                entered("hubspot_private_app", "b", none()),
                "requires scope 'b'",
            ),
            (
                entered("hubspot_private_app", "", serde_json::json!([])),
                "not an object",
            ),
        ] {
            let error = check_app(&bad).unwrap_err();
            assert!(error.contains(why), "{error}");
        }
    }

    #[test]
    fn a_connection_asks_for_the_apps_scopes_or_fewer() {
        let app = app("hubspot", "hubspot_private_app");
        let default = "oauth crm.objects.contacts.read crm.objects.contacts.write";
        assert_eq!(scopes_for(&app, None, default).unwrap(), app.scopes);
        assert_eq!(scopes_for(&app, Some(" "), default).unwrap(), app.scopes);
        assert_eq!(
            scopes_for(&app, Some(default), default).unwrap(),
            app.scopes
        );
        // A subset, with the required scope added.
        assert_eq!(
            scopes_for(&app, Some("crm.objects.deals.read"), default).unwrap(),
            "crm.objects.deals.read oauth"
        );
        let error = scopes_for(&app, Some("crm.objects.contacts.write"), default).unwrap_err();
        assert!(error.contains("'crm.objects.contacts.write'"), "{error}");
    }

    #[test]
    fn a_providers_error_is_told_apart_for_alerts() {
        for (message, kind) in [
            (
                "Token endpoint returned 429 Too Many Requests: unknown_error",
                "rate_limited",
            ),
            (
                "token endpoint returned 401 Unauthorized: invalid_client bad secret",
                "invalid_client",
            ),
            (
                "Token endpoint returned 400 Bad Request: invalid_grant expired",
                "invalid_grant",
            ),
            (
                "Token endpoint returned 503 Service Unavailable: unknown_error",
                "provider_unavailable",
            ),
            ("error sending request", "other"),
        ] {
            assert_eq!(provider_error_kind(message), kind, "{message}");
        }
    }

    struct One(Arc<OAuthAppCredentials>);
    impl OAuthApps for One {
        fn get(&self, tenant: &str, app_id: &str) -> Option<Arc<OAuthAppCredentials>> {
            (tenant == "acme" && app_id == self.0.app_id).then(|| self.0.clone())
        }
        fn available(&self, _tenant: &str) -> Vec<OAuthAppSummary> {
            vec![self.0.as_ref().into()]
        }
    }

    #[test]
    fn an_app_is_resolved_only_for_its_tenant_and_type() {
        let apps = One(Arc::new(app("hubspot", "hubspot_private_app")));
        let params = serde_json::json!({ "oauth_app": "hubspot" });
        assert!(
            app_for(Some(&apps), "acme", "hubspot_private_app", &params)
                .unwrap()
                .is_some()
        );
        assert!(app_for(Some(&apps), "beta", "hubspot_private_app", &params).is_err());
        assert!(app_for(Some(&apps), "acme", "quickbooks_online", &params).is_err());
        assert!(app_for(None, "acme", "hubspot_private_app", &params).is_err());
        assert!(
            app_for(
                Some(&apps),
                "acme",
                "hubspot_private_app",
                &serde_json::json!({})
            )
            .unwrap()
            .is_none()
        );
    }
}
