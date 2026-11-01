//! OAuth apps the platform registered with a provider once, for tenants to
//! connect with instead of registering their own.
//!
//! A connection names one in its `oauth_app` parameter in place of a client
//! ID and secret. Its credentials are resolved where a request to the
//! provider needs them, and added only to that request's view of the
//! connection ([`with_app_credentials`]); they are never written into the
//! connection, so nothing that stores the connection back can persist them.
//!
//! The host supplies the apps ([`OAuthApps`]); runtara reads them from the
//! file `RUNTARA_OAUTH_APPS_FILE` names ([`FileOAuthApps`]) when it supplies
//! none.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, LazyLock};

use opentelemetry::KeyValue;
use opentelemetry::metrics::Counter;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use zeroize::Zeroizing;

/// The reserved connection parameter naming the platform app a connection
/// uses.
pub const OAUTH_APP_PARAM: &str = "oauth_app";

/// A platform app's credentials.
pub struct OAuthAppCredentials {
    pub app_id: String,
    /// The one connection type it may serve.
    pub integration_id: String,
    pub display_name: String,
    pub client_id: String,
    pub client_secret: Zeroizing<String>,
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

/// The settings that name the apps file.
pub const OAUTH_APPS_FILE_ENV: &str = "RUNTARA_OAUTH_APPS_FILE";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AppsFile {
    apps: Vec<AppEntry>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AppEntry {
    id: String,
    integration_id: String,
    display_name: String,
    client_id: String,
    client_secret: String,
    scopes: String,
    #[serde(default)]
    required_scopes: String,
}

/// Platform apps from a JSON file, every one available to every tenant:
///
/// ```json
/// {"apps": [{"id": "hubspot", "integrationId": "hubspot_private_app",
///            "displayName": "HubSpot", "clientId": "…", "clientSecret": "…",
///            "scopes": "oauth crm.objects.contacts.read", "requiredScopes": "oauth"}]}
/// ```
pub struct FileOAuthApps {
    apps: HashMap<String, Arc<OAuthAppCredentials>>,
}

impl FileOAuthApps {
    /// The apps in `json`. Refuses an app for a connection type that takes
    /// its endpoints from the connection (a platform secret must never go to
    /// an endpoint a tenant chose), and one whose required scopes it was not
    /// registered for.
    pub fn from_json(json: &str) -> Result<Self, String> {
        let file: AppsFile = serde_json::from_str(json)
            .map_err(|error| format!("the OAuth apps are not valid: {error}"))?;
        let mut apps = HashMap::new();
        for entry in file.apps {
            let app = OAuthAppCredentials {
                app_id: entry.id,
                integration_id: entry.integration_id,
                display_name: entry.display_name,
                client_id: entry.client_id,
                client_secret: Zeroizing::new(entry.client_secret),
                scopes: entry.scopes,
                required_scopes: entry.required_scopes,
            };
            check_app(&app)?;
            if apps.insert(app.app_id.clone(), Arc::new(app)).is_some() {
                return Err("the OAuth apps name one app twice".to_string());
            }
        }
        Ok(Self { apps })
    }

    /// The apps in the file `RUNTARA_OAUTH_APPS_FILE` names; `None` when it
    /// names none.
    pub fn from_env() -> Result<Option<Self>, String> {
        let Some(path) = std::env::var_os(OAUTH_APPS_FILE_ENV).filter(|path| !path.is_empty())
        else {
            return Ok(None);
        };
        let json = Zeroizing::new(std::fs::read_to_string(&path).map_err(|error| {
            format!(
                "{OAUTH_APPS_FILE_ENV}: cannot read {}: {error}",
                path.to_string_lossy()
            )
        })?);
        Self::from_json(&json)
            .map(Some)
            .map_err(|error| format!("{OAUTH_APPS_FILE_ENV}: {error}"))
    }
}

impl OAuthApps for FileOAuthApps {
    fn get(&self, _tenant: &str, app_id: &str) -> Option<Arc<OAuthAppCredentials>> {
        self.apps.get(app_id).cloned()
    }

    fn available(&self, _tenant: &str) -> Vec<OAuthAppSummary> {
        let mut apps: Vec<_> = self.apps.values().map(|app| app.as_ref().into()).collect();
        apps.sort_by(|a: &OAuthAppSummary, b| a.id.cmp(&b.id));
        apps
    }
}

/// Whether `app` can serve a connection at all.
pub fn check_app(app: &OAuthAppCredentials) -> Result<(), String> {
    let id = &app.app_id;
    if id.is_empty() || app.client_id.is_empty() || app.client_secret.is_empty() {
        return Err(format!(
            "OAuth app '{id}' needs an id, a client ID and a client secret"
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

/// `params` as a request to the provider sees them: with the app's client
/// credentials, and its scopes. Only ever passed to an outgoing call, never
/// written back.
pub fn with_app_credentials(params: &Value, app: &OAuthAppCredentials) -> Value {
    let mut effective = params.clone();
    if let Some(obj) = effective.as_object_mut() {
        obj.insert("client_id".into(), Value::String(app.client_id.clone()));
        obj.insert(
            "client_secret".into(),
            Value::String(app.client_secret.as_str().to_owned()),
        );
    }
    effective
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn app(id: &str, integration: &str) -> OAuthAppCredentials {
        OAuthAppCredentials {
            app_id: id.into(),
            integration_id: integration.into(),
            display_name: "HubSpot".into(),
            client_id: "platform-client".into(),
            client_secret: Zeroizing::new("platform-secret".into()),
            scopes: "oauth crm.objects.contacts.read crm.objects.deals.read".into(),
            required_scopes: "oauth".into(),
        }
    }

    #[test]
    fn the_file_is_read_and_checked() {
        let apps = FileOAuthApps::from_json(
            r#"{"apps":[{"id":"hubspot","integrationId":"hubspot_private_app",
                "displayName":"HubSpot","clientId":"c","clientSecret":"s",
                "scopes":"oauth crm.objects.contacts.read","requiredScopes":"oauth"}]}"#,
        )
        .unwrap();
        let hubspot = apps.get("any-tenant", "hubspot").unwrap();
        assert_eq!(hubspot.client_id, "c");
        assert_eq!(apps.available("any-tenant")[0].id, "hubspot");
        assert!(!format!("{hubspot:?}").contains("client_secret"));

        for (bad, why) in [
            (
                r#"{"apps":[{"id":"g","integrationId":"http_oauth2_authorization_code","displayName":"G","clientId":"c","clientSecret":"s","scopes":"a"}]}"#,
                "takes its endpoints",
            ),
            (
                r#"{"apps":[{"id":"x","integrationId":"stripe","displayName":"X","clientId":"c","clientSecret":"s","scopes":"a"}]}"#,
                "not an authorization-code",
            ),
            (
                r#"{"apps":[{"id":"h","integrationId":"hubspot_private_app","displayName":"H","clientId":"c","clientSecret":"s","scopes":"a","requiredScopes":"b"}]}"#,
                "requires scope 'b'",
            ),
            (r#"{"apps":[{"id":"h"}]}"#, "not valid"),
        ] {
            let error = FileOAuthApps::from_json(bad).err().unwrap();
            assert!(error.contains(why), "{error}");
            assert!(!error.contains("\"s\""), "no secret in {error}");
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

        let effective = with_app_credentials(&params, &apps.0);
        assert_eq!(effective["client_secret"], "platform-secret");
        assert!(
            params.get("client_secret").is_none(),
            "the original is unchanged"
        );
    }
}
