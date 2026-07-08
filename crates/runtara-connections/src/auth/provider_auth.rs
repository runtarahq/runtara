use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use reqwest::Client;
use runtara_agents::registry::find_connection_type;
use runtara_dsl::agent_meta::{OAuthConfig, TokenEndpointAuth};
use serde_json::Value;
use std::collections::HashMap;

use super::aws_signing::AwsSigningParams;
use super::azure_signing::AzureSigningParams;
use super::token_cache::{
    self, DEFAULT_CLIENT_CREDENTIALS_TTL_SECONDS, DeferredAuth, TokenRequestBody,
};

pub struct ResolvedConnectionAuth {
    pub base_url: Option<String>,
    pub aws_signing: Option<AwsSigningParams>,
    pub azure_signing: Option<AzureSigningParams>,
    /// Credentials produced by an actual OAuth refresh during this resolution that
    /// must be persisted back to the connection. `None` on every cache hit / fast
    /// path and for non-refresh grants. Consumed by the facade write-back.
    pub rotated_credentials: Option<RotatedCredentials>,
}

/// Access + (possibly rotated) refresh token captured when an OAuth refresh fires,
/// to be sealed back into `connection_parameters`. Has a hand-written `Debug` that
/// redacts the token values so a stray `{:?}` can't leak them to logs.
pub struct RotatedCredentials {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub token_expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl std::fmt::Debug for RotatedCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RotatedCredentials")
            .field("access_token", &"<redacted>")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .field("token_expires_at", &self.token_expires_at)
            .finish()
    }
}

pub(crate) struct ConnectionAuthDescriptor {
    pub base_url: Option<String>,
    pub aws_signing: Option<AwsSigningParams>,
    pub azure_signing: Option<AzureSigningParams>,
    pub deferred_auth: Option<DeferredAuth>,
}

const SHOPIFY_SCOPE_FIELDS: &[(&str, &str)] = &[
    ("scope_read_products", "read_products"),
    ("scope_write_products", "write_products"),
    ("scope_read_orders", "read_orders"),
    ("scope_write_orders", "write_orders"),
    ("scope_read_inventory", "read_inventory"),
    ("scope_write_inventory", "write_inventory"),
    ("scope_read_locations", "read_locations"),
    ("scope_read_customers", "read_customers"),
    ("scope_write_customers", "write_customers"),
    ("scope_read_fulfillments", "read_fulfillments"),
    ("scope_write_fulfillments", "write_fulfillments"),
];

pub async fn resolve_connection_auth(
    client: &Client,
    connection_id: &str,
    integration_id: &str,
    params: &Value,
    headers: &mut HashMap<String, String>,
    events: &crate::events::ConnectionEvents,
) -> Result<ResolvedConnectionAuth, String> {
    let descriptor = describe_connection_auth(connection_id, integration_id, params, headers);

    let mut rotated_credentials = None;
    if let Some(deferred_auth) = descriptor.deferred_auth {
        let resolved = token_cache::resolve_deferred_auth(
            client,
            deferred_auth,
            events,
            connection_id,
            integration_id,
        )
        .await?;
        headers.insert(resolved.header_name, resolved.header_value);
        rotated_credentials = resolved.rotated;
    }

    Ok(ResolvedConnectionAuth {
        base_url: descriptor.base_url,
        aws_signing: descriptor.aws_signing,
        azure_signing: descriptor.azure_signing,
        rotated_credentials,
    })
}

fn describe_connection_auth(
    connection_id: &str,
    integration_id: &str,
    params: &Value,
    headers: &mut HashMap<String, String>,
) -> ConnectionAuthDescriptor {
    match integration_id {
        "openai_api_key" => {
            if let Some(key) = params["api_key"].as_str() {
                headers.insert("Authorization".into(), format!("Bearer {}", key));
            }
            ConnectionAuthDescriptor {
                base_url: Some("https://api.openai.com".into()),
                aws_signing: None,
                azure_signing: None,
                deferred_auth: None,
            }
        }
        "shopify_access_token" => {
            if let Some(token) = params["access_token"].as_str() {
                headers.insert("X-Shopify-Access-Token".into(), token.to_string());
            }
            ConnectionAuthDescriptor {
                base_url: params["shop_domain"]
                    .as_str()
                    .map(|domain| format!("https://{}", domain)),
                aws_signing: None,
                azure_signing: None,
                deferred_auth: None,
            }
        }
        "shopify_client_credentials" => ConnectionAuthDescriptor {
            base_url: params["shop_domain"]
                .as_str()
                .map(|domain| format!("https://{}", domain)),
            aws_signing: None,
            azure_signing: None,
            deferred_auth: describe_shopify_client_credentials_auth(connection_id, params),
        },
        "microsoft_entra_client_credentials" => ConnectionAuthDescriptor {
            base_url: first_string_param(params, &["base_url"]),
            aws_signing: None,
            azure_signing: None,
            deferred_auth: describe_microsoft_entra_client_credentials_auth(connection_id, params),
        },
        // Generic bring-your-own-endpoint OAuth2 client-credentials (M2M): every
        // endpoint/config value comes from the connection's own parameters. The mint
        // request goes through the hardened egress client (no redirects, DNS-guarded).
        "http_oauth2_client_credentials" => ConnectionAuthDescriptor {
            base_url: first_string_param(params, &["base_url"]),
            aws_signing: None,
            azure_signing: None,
            deferred_auth: describe_http_oauth2_client_credentials_auth(connection_id, params),
        },
        "hubspot_access_token" => {
            if let Some(token) = params["access_token"].as_str() {
                headers.insert("Authorization".into(), format!("Bearer {}", token));
            }
            ConnectionAuthDescriptor {
                base_url: Some("https://api.hubapi.com".into()),
                aws_signing: None,
                azure_signing: None,
                deferred_auth: None,
            }
        }
        // OAuth2 authorization-code integrations are fully descriptor-driven: base URL,
        // token-endpoint auth style, refresh rotation and extra callback params all come
        // from the connection type's OAuthConfig (see runtara-dsl agent_meta).
        "hubspot_private_app" | "quickbooks_online" => {
            describe_oauth_authcode_auth(connection_id, integration_id, params, headers)
        }
        "stripe_api_key" => {
            if let Some(key) = params["secret_key"].as_str() {
                headers.insert("Authorization".into(), format!("Bearer {}", key));
            }
            ConnectionAuthDescriptor {
                base_url: Some("https://api.stripe.com".into()),
                aws_signing: None,
                azure_signing: None,
                deferred_auth: None,
            }
        }
        "slack_bot" => {
            if let Some(token) = params["bot_token"].as_str() {
                headers.insert("Authorization".into(), format!("Bearer {}", token));
            }
            ConnectionAuthDescriptor {
                base_url: Some("https://slack.com".into()),
                aws_signing: None,
                azure_signing: None,
                deferred_auth: None,
            }
        }
        "mailgun" => {
            if let Some(key) = params["api_key"].as_str() {
                let encoded = BASE64.encode(format!("api:{}", key));
                headers.insert("Authorization".into(), format!("Basic {}", encoded));
            }
            let region = params["region"].as_str().unwrap_or("us");
            let base = match region {
                "eu" => "https://api.eu.mailgun.net".to_string(),
                _ => "https://api.mailgun.net".to_string(),
            };
            ConnectionAuthDescriptor {
                base_url: Some(base),
                aws_signing: None,
                azure_signing: None,
                deferred_auth: None,
            }
        }
        // ── HTTP Bearer token ────────────────────────────────────
        "http_bearer" => {
            if let Some(token) = params["token"].as_str() {
                headers.insert("Authorization".into(), format!("Bearer {}", token));
            }
            ConnectionAuthDescriptor {
                base_url: params["base_url"].as_str().map(|u| u.to_string()),
                aws_signing: None,
                azure_signing: None,
                deferred_auth: None,
            }
        }
        // ── HTTP API Key ────────────────────────────────────────
        "http_api_key" => {
            if let Some(key) = params["api_key"].as_str() {
                let header_name = params["header_name"]
                    .as_str()
                    .unwrap_or("X-API-Key")
                    .to_string();
                headers.insert(header_name, key.to_string());
            }
            ConnectionAuthDescriptor {
                base_url: params["base_url"].as_str().map(|u| u.to_string()),
                aws_signing: None,
                azure_signing: None,
                deferred_auth: None,
            }
        }
        // ── MCP (Model Context Protocol) ─────────────────────────
        // The agent (runtara-agent-mcp) sends its JSON-RPC bodies through
        // the proxy; this arm injects the right Authorization / api-key
        // header for the three v1 auth modes. OAuth2 is reserved for a
        // follow-up — extend `auth_mode` then.
        "mcp" => {
            let auth_mode = params["auth_mode"].as_str().unwrap_or("none");
            match auth_mode {
                "bearer" => {
                    if let Some(token) = params["bearer_token"].as_str() {
                        headers.insert("Authorization".into(), format!("Bearer {}", token));
                    }
                }
                "api_key" => {
                    if let Some(value) = params["api_key_value"].as_str() {
                        let header_name = params["api_key_header"]
                            .as_str()
                            .filter(|s| !s.is_empty())
                            .unwrap_or("X-API-Key")
                            .to_string();
                        headers.insert(header_name, value.to_string());
                    }
                }
                _ => { /* "none" — no auth header injected */ }
            }
            // Extra static headers configured on the connection.
            if let Some(extras) = params["extra_headers"].as_object() {
                for (k, v) in extras {
                    if let Some(s) = v.as_str() {
                        headers.insert(k.clone(), s.to_string());
                    }
                }
            }
            ConnectionAuthDescriptor {
                base_url: params["url"].as_str().map(|u| u.to_string()),
                aws_signing: None,
                azure_signing: None,
                deferred_auth: None,
            }
        }
        "aws_credentials" | "s3_compatible" => {
            let access_key_id = first_string_param(params, &["access_key_id", "aws_access_key_id"])
                .unwrap_or_default();
            let secret_access_key =
                first_string_param(params, &["secret_access_key", "aws_secret_access_key"])
                    .unwrap_or_default();
            let region = first_string_param(params, &["region", "aws_region"])
                .unwrap_or_else(|| "us-east-1".to_string());
            let session_token = first_string_param(params, &["session_token", "aws_session_token"]);

            let (base_url, service) = if integration_id == "s3_compatible" {
                let endpoint = params["endpoint"]
                    .as_str()
                    .map(normalize_endpoint)
                    .unwrap_or_else(|| format!("https://s3.{}.amazonaws.com", region));
                (Some(endpoint), "s3".to_string())
            } else {
                let svc = params["service"].as_str().unwrap_or("bedrock").to_string();
                let base = params["endpoint"]
                    .as_str()
                    .map(normalize_endpoint)
                    .or_else(|| {
                        (svc == "bedrock")
                            .then(|| format!("https://bedrock-runtime.{}.amazonaws.com", region))
                    });
                (base, svc)
            };

            ConnectionAuthDescriptor {
                base_url,
                aws_signing: Some(AwsSigningParams {
                    access_key_id,
                    secret_access_key,
                    region,
                    service,
                    session_token,
                }),
                azure_signing: None,
                deferred_auth: None,
            }
        }
        "azure_blob_storage" => {
            let account_name = first_string_param(params, &["account_name"]).unwrap_or_default();
            let account_key = first_string_param(params, &["account_key"]).unwrap_or_default();
            let base_url = resolve_azure_blob_base_url(params, &account_name);

            ConnectionAuthDescriptor {
                base_url,
                aws_signing: None,
                azure_signing: Some(AzureSigningParams {
                    account_name,
                    account_key,
                }),
                deferred_auth: None,
            }
        }
        _ => {
            if let Some(key) = params["api_key"].as_str() {
                headers
                    .entry("Authorization".into())
                    .or_insert_with(|| format!("Bearer {}", key));
            } else if let Some(token) = params["access_token"].as_str() {
                headers
                    .entry("Authorization".into())
                    .or_insert_with(|| format!("Bearer {}", token));
            }
            ConnectionAuthDescriptor {
                base_url: params["base_url"].as_str().map(|u| u.to_string()),
                aws_signing: None,
                azure_signing: None,
                deferred_auth: None,
            }
        }
    }
}

fn describe_shopify_client_credentials_auth(
    connection_id: &str,
    params: &Value,
) -> Option<DeferredAuth> {
    let shop_domain = params["shop_domain"].as_str()?.trim_end_matches('/');
    let client_id = params["client_id"].as_str()?;
    let client_secret = params["client_secret"].as_str()?;

    let scopes = collect_shopify_scopes(params);
    let mut body = serde_json::Map::new();
    body.insert(
        "client_id".to_string(),
        Value::String(client_id.to_string()),
    );
    body.insert(
        "client_secret".to_string(),
        Value::String(client_secret.to_string()),
    );
    body.insert(
        "grant_type".to_string(),
        Value::String("client_credentials".to_string()),
    );
    if !scopes.is_empty() {
        body.insert("scope".to_string(), Value::String(scopes.clone()));
    }

    Some(DeferredAuth::OAuth2ClientCredentials {
        cache_key: token_cache::build_token_cache_key(&[
            "shopify_client_credentials",
            connection_id,
            shop_domain,
            &scopes,
        ]),
        token_url: format!("https://{shop_domain}/admin/oauth/access_token"),
        header_name: "X-Shopify-Access-Token".to_string(),
        header_value_prefix: None,
        request_body: TokenRequestBody::Json(Value::Object(body)),
        basic_auth: None,
        default_ttl_seconds: DEFAULT_CLIENT_CREDENTIALS_TTL_SECONDS,
    })
}

fn describe_microsoft_entra_client_credentials_auth(
    connection_id: &str,
    params: &Value,
) -> Option<DeferredAuth> {
    let _base_url = first_string_param(params, &["base_url"])?;
    let tenant_id = first_string_param(params, &["tenant_id"])?
        .trim_matches('/')
        .to_string();
    let client_id = first_string_param(params, &["client_id"])?;
    let client_secret = first_string_param(params, &["client_secret"])?;
    let scope = first_string_param(params, &["scope"])?;
    let authority_host = first_string_param(params, &["authority_host"])
        .unwrap_or_else(|| "https://login.microsoftonline.com".to_string())
        .trim_end_matches('/')
        .to_string();

    Some(DeferredAuth::OAuth2ClientCredentials {
        cache_key: token_cache::build_token_cache_key(&[
            "microsoft_entra_client_credentials",
            connection_id,
            &authority_host,
            &tenant_id,
            &client_id,
            &scope,
        ]),
        token_url: format!("{authority_host}/{tenant_id}/oauth2/v2.0/token"),
        header_name: "Authorization".to_string(),
        header_value_prefix: Some("Bearer ".to_string()),
        request_body: TokenRequestBody::FormUrlEncoded(vec![
            ("grant_type".to_string(), "client_credentials".to_string()),
            ("client_id".to_string(), client_id),
            ("client_secret".to_string(), client_secret),
            ("scope".to_string(), scope),
        ]),
        basic_auth: None,
        default_ttl_seconds: DEFAULT_CLIENT_CREDENTIALS_TTL_SECONDS,
    })
}

/// Effective OAuth flow config. THE GATE IS PER-TYPE, NOT PER-FIELD: when
/// `cfg.params_driven == false` (every curated provider), connection params are
/// ignored for ALL fields — including ones the curated descriptor legitimately
/// leaves empty (e.g. HubSpot has no revocation endpoint; a per-field-emptiness
/// fallback would let a hostile `revocation_url` param exfiltrate the refresh
/// token on disconnect). Only the generic bring-your-own types read params.
pub(crate) struct EffectiveOAuthConfig {
    pub auth_url: String,
    pub token_url: String,
    pub token_endpoint_auth: TokenEndpointAuth,
    pub pkce_required: bool,
    pub refresh_token_rotates: bool,
    pub revocation_endpoint: String,
}

pub(crate) fn resolve_effective_oauth_config(
    cfg: &OAuthConfig,
    params: &Value,
) -> EffectiveOAuthConfig {
    if !cfg.params_driven {
        return EffectiveOAuthConfig {
            auth_url: cfg.auth_url.to_string(),
            token_url: cfg.token_url.to_string(),
            token_endpoint_auth: cfg.token_endpoint_auth,
            pkce_required: cfg.pkce_required,
            refresh_token_rotates: cfg.refresh_token_rotates,
            revocation_endpoint: cfg.revocation_endpoint.to_string(),
        };
    }
    EffectiveOAuthConfig {
        auth_url: first_string_param(params, &["auth_url"]).unwrap_or_default(),
        token_url: first_string_param(params, &["token_url"]).unwrap_or_default(),
        token_endpoint_auth: if params["token_auth"].as_str() == Some("basic") {
            TokenEndpointAuth::HttpBasic
        } else {
            TokenEndpointAuth::FormBody
        },
        // PKCE defaults ON for params-driven types; `pkce = false` disables it
        // for providers that reject code_challenge.
        pkce_required: params["pkce"].as_bool().unwrap_or(true),
        // Fail-closed by default: a rotated-token persist failure must fail the
        // mint rather than silently losing the rotated token.
        refresh_token_rotates: params["refresh_rotates"].as_bool().unwrap_or(true),
        revocation_endpoint: first_string_param(params, &["revocation_url"]).unwrap_or_default(),
    }
}

/// Descriptor-driven auth for OAuth2 authorization-code integrations: resolves the
/// base URL from the connection type's `OAuthConfig` and builds the refresh-token
/// deferred auth. Falls back to injecting the stored `access_token` directly when no
/// refresh token is present yet (immediately post-consent, before the first refresh).
fn describe_oauth_authcode_auth(
    connection_id: &str,
    integration_id: &str,
    params: &Value,
    headers: &mut HashMap<String, String>,
) -> ConnectionAuthDescriptor {
    let base_url = find_connection_type(integration_id)
        .and_then(|meta| meta.oauth_config)
        .and_then(|cfg| resolve_oauth_base_url(cfg, params));

    match describe_oauth_refresh_auth(connection_id, integration_id, params, "Authorization") {
        Some(auth) => ConnectionAuthDescriptor {
            base_url,
            aws_signing: None,
            azure_signing: None,
            deferred_auth: Some(auth),
        },
        None => {
            if let Some(token) = params["access_token"].as_str() {
                headers.insert("Authorization".into(), format!("Bearer {}", token));
            }
            ConnectionAuthDescriptor {
                base_url,
                aws_signing: None,
                azure_signing: None,
                deferred_auth: None,
            }
        }
    }
}

/// Resolve an OAuth integration's API base URL from its descriptor: pick the sandbox
/// host when the connection's `environment` param is `"sandbox"`, then append any
/// `{param}`-templated path (e.g. QuickBooks `/v3/company/{realm_id}`). Returns `None`
/// when the descriptor declares no base URL.
fn resolve_oauth_base_url(cfg: &OAuthConfig, params: &Value) -> Option<String> {
    // Params-driven (generic) types: the base URL is the connection's own.
    if cfg.params_driven {
        return first_string_param(params, &["base_url"])
            .map(|u| u.trim_end_matches('/').to_string());
    }
    let is_sandbox = params["environment"].as_str() == Some("sandbox");
    let host = if !cfg.sandbox_base_url.is_empty() && is_sandbox {
        cfg.sandbox_base_url
    } else {
        cfg.base_url
    };
    if host.is_empty() {
        return None;
    }
    let mut url = host.trim_end_matches('/').to_string();
    if !cfg.base_url_path_template.is_empty() {
        url.push_str(&substitute_path_template(
            cfg.base_url_path_template,
            params,
        ));
    }
    Some(url)
}

/// Replace `{name}` placeholders in a path template with the matching string param.
/// A missing param substitutes empty (yielding an obviously-broken path the API
/// rejects clearly, rather than silently hitting the wrong resource).
fn substitute_path_template(template: &str, params: &Value) -> String {
    let mut out = String::with_capacity(template.len());
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '{' {
            let mut name = String::new();
            for c2 in chars.by_ref() {
                if c2 == '}' {
                    break;
                }
                name.push(c2);
            }
            if let Some(val) = params[name.as_str()].as_str() {
                out.push_str(val);
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Generic OAuth2 client-credentials mint, fully parameter-driven. `token_auth =
/// "basic"` sends the credentials as an HTTP Basic header and keeps them OUT of
/// the form body (Okta-style); the default puts them in the body (RFC 6749).
/// The cache key folds in token_url + base_url so editing either endpoint on the
/// connection naturally misses any stale cached token.
fn describe_http_oauth2_client_credentials_auth(
    connection_id: &str,
    params: &Value,
) -> Option<DeferredAuth> {
    let token_url = first_string_param(params, &["token_url"])?;
    let client_id = first_string_param(params, &["client_id"])?;
    let client_secret = first_string_param(params, &["client_secret"])?;
    let scope = first_string_param(params, &["scope"]).unwrap_or_default();
    let base_url = first_string_param(params, &["base_url"]).unwrap_or_default();
    let use_basic = params["token_auth"].as_str() == Some("basic");

    let mut fields = vec![("grant_type".to_string(), "client_credentials".to_string())];
    if !use_basic {
        fields.push(("client_id".to_string(), client_id.clone()));
        fields.push(("client_secret".to_string(), client_secret.clone()));
    }
    if !scope.is_empty() {
        fields.push(("scope".to_string(), scope.clone()));
    }
    if let Some(audience) = first_string_param(params, &["audience"]) {
        fields.push(("audience".to_string(), audience));
    }
    if let Some(resource) = first_string_param(params, &["resource"]) {
        fields.push(("resource".to_string(), resource));
    }

    Some(DeferredAuth::OAuth2ClientCredentials {
        cache_key: token_cache::build_token_cache_key(&[
            "http_oauth2_client_credentials",
            connection_id,
            &token_url,
            &base_url,
            &client_id,
            &scope,
        ]),
        token_url,
        header_name: "Authorization".to_string(),
        header_value_prefix: Some("Bearer ".to_string()),
        request_body: TokenRequestBody::FormUrlEncoded(fields),
        basic_auth: use_basic.then_some((client_id, client_secret)),
        default_ttl_seconds: DEFAULT_CLIENT_CREDENTIALS_TTL_SECONDS,
    })
}

fn describe_oauth_refresh_auth(
    connection_id: &str,
    integration_id: &str,
    params: &Value,
    header_name: &str,
) -> Option<DeferredAuth> {
    let refresh_token = params["refresh_token"].as_str()?.to_string();
    let client_id = params["client_id"].as_str()?.to_string();
    let client_secret = params["client_secret"].as_str()?.to_string();
    let oauth_config = find_connection_type(integration_id)?.oauth_config?;
    let effective = resolve_effective_oauth_config(oauth_config, params);
    if effective.token_url.is_empty() {
        // Fail fast instead of refreshing against an empty-host URL.
        return None;
    }

    Some(DeferredAuth::OAuth2RefreshToken {
        // token_url in the key so an endpoint edit on a params-driven connection
        // naturally misses stale cache entries.
        cache_key: token_cache::build_token_cache_key(&[
            "oauth_refresh",
            connection_id,
            integration_id,
            &effective.token_url,
        ]),
        token_url: effective.token_url,
        header_name: header_name.to_string(),
        client_id,
        client_secret,
        refresh_token,
        token_endpoint_auth: effective.token_endpoint_auth,
        fallback_access_token: params["access_token"].as_str().map(|s| s.to_string()),
        fallback_expires_at: token_cache::parse_expiry(params["token_expires_at"].as_str()),
    })
}

/// Build the `(optional Basic auth header, JSON body)` for a token-revocation
/// request from the descriptor + connection params. Returns `None` when there is no
/// revocation endpoint or no token to revoke. Sends `{"token": <refresh_or_access>}`;
/// providers that authenticate the revoke with HTTP Basic (Intuit) get the header.
pub(crate) fn build_revoke_request(
    oauth_config: &OAuthConfig,
    params: &Value,
) -> Option<(Option<String>, String, String)> {
    let effective = resolve_effective_oauth_config(oauth_config, params);
    if effective.revocation_endpoint.is_empty() {
        return None;
    }
    let token = params["refresh_token"]
        .as_str()
        .or_else(|| params["access_token"].as_str())?;
    let body = serde_json::json!({ "token": token }).to_string();
    let basic = match effective.token_endpoint_auth {
        TokenEndpointAuth::HttpBasic => {
            let client_id = params["client_id"].as_str().unwrap_or("");
            let client_secret = params["client_secret"].as_str().unwrap_or("");
            Some(format!(
                "Basic {}",
                BASE64.encode(format!("{client_id}:{client_secret}"))
            ))
        }
        TokenEndpointAuth::FormBody => None,
    };
    Some((basic, body, effective.revocation_endpoint))
}

/// Best-effort provider-side token revocation, called on disconnect. A no-op when the
/// effective config declares no revocation endpoint or the connection has no token.
pub async fn revoke_oauth_token(
    client: &Client,
    oauth_config: &OAuthConfig,
    params: &Value,
) -> Result<(), String> {
    let Some((basic_auth, body, endpoint)) = build_revoke_request(oauth_config, params) else {
        return Ok(());
    };
    let mut request = client
        .post(endpoint)
        .header("Content-Type", "application/json")
        .body(body)
        .timeout(std::time::Duration::from_secs(10));
    if let Some(header) = basic_auth {
        request = request.header("Authorization", header);
    }
    let response = request
        .send()
        .await
        .map_err(|e| format!("revocation request failed: {e}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "revocation endpoint returned {}",
            response.status()
        ));
    }
    Ok(())
}

fn collect_shopify_scopes(params: &Value) -> String {
    SHOPIFY_SCOPE_FIELDS
        .iter()
        .filter(|(field, _)| params[*field].as_bool().unwrap_or(false))
        .map(|(_, scope)| *scope)
        .collect::<Vec<_>>()
        .join(",")
}

fn first_string_param(params: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        params[*key]
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

fn normalize_endpoint(endpoint: &str) -> String {
    if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
        endpoint.to_string()
    } else {
        format!("https://{}", endpoint)
    }
}

/// Default regional endpoint for a uniform AWS service host,
/// `https://{service}.{region}.amazonaws.com`. Correct for the many services
/// that follow the regular naming scheme (SQS, SNS, DynamoDB, Lambda, Kinesis,
/// regional STS, …). Irregular hosts (bedrock-runtime, S3 virtual-host, global
/// services like IAM, FIPS/dualstack variants) must instead set an explicit
/// `endpoint` on the connection. Used by the proxy when an agent declares its
/// AWS service and the connection pinned no explicit endpoint.
pub fn aws_default_endpoint(service: &str, region: &str) -> String {
    format!("https://{service}.{region}.amazonaws.com")
}

fn resolve_azure_blob_base_url(params: &Value, account_name: &str) -> Option<String> {
    if let Some(raw) = first_string_param(params, &["endpoint_override", "endpoint"]) {
        let trimmed = raw.trim_end_matches('/').to_string();
        let normalized = normalize_endpoint(&trimmed);
        // For path-style endpoints (Azurite/Azure Stack), append the account when
        // it isn't already part of the configured URL.
        if normalized.ends_with(&format!("/{}", account_name)) || account_name.is_empty() {
            return Some(normalized);
        }
        return Some(format!(
            "{}/{}",
            normalized.trim_end_matches('/'),
            account_name
        ));
    }

    if account_name.is_empty() {
        return None;
    }

    let suffix = first_string_param(params, &["endpoint_suffix"])
        .unwrap_or_else(|| "core.windows.net".to_string());
    Some(format!(
        "https://{}.blob.{}",
        account_name,
        suffix.trim_start_matches('.').trim_end_matches('/')
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtara_dsl::agent_meta::TokenEndpointAuth;
    use serde_json::json;

    fn oauth_cfg(
        base_url: &'static str,
        sandbox_base_url: &'static str,
        path_template: &'static str,
    ) -> OAuthConfig {
        OAuthConfig {
            auth_url: "",
            token_url: "",
            default_scopes: "",
            token_endpoint_auth: TokenEndpointAuth::FormBody,
            refresh_token_rotates: false,
            base_url,
            sandbox_base_url,
            base_url_path_template: path_template,
            extra_callback_params: &[],
            reauth_on_error_codes: &[],
            revocation_endpoint: "",
            pkce_required: false,
            params_driven: false,
        }
    }

    #[test]
    fn resolve_oauth_base_url_static_host() {
        let cfg = oauth_cfg("https://api.hubapi.com", "", "");
        assert_eq!(
            resolve_oauth_base_url(&cfg, &json!({})).as_deref(),
            Some("https://api.hubapi.com")
        );
    }

    #[test]
    fn resolve_oauth_base_url_none_when_no_host() {
        let cfg = oauth_cfg("", "", "");
        assert_eq!(resolve_oauth_base_url(&cfg, &json!({})), None);
    }

    #[test]
    fn resolve_oauth_base_url_env_select_and_template() {
        let cfg = oauth_cfg(
            "https://quickbooks.api.intuit.com",
            "https://sandbox-quickbooks.api.intuit.com",
            "/v3/company/{realm_id}",
        );
        // production host + realm path
        assert_eq!(
            resolve_oauth_base_url(&cfg, &json!({"environment":"production","realm_id":"123"}))
                .as_deref(),
            Some("https://quickbooks.api.intuit.com/v3/company/123")
        );
        // sandbox host selected by the environment param
        assert_eq!(
            resolve_oauth_base_url(&cfg, &json!({"environment":"sandbox","realm_id":"123"}))
                .as_deref(),
            Some("https://sandbox-quickbooks.api.intuit.com/v3/company/123")
        );
        // no environment param → defaults to the production host
        assert_eq!(
            resolve_oauth_base_url(&cfg, &json!({"realm_id":"456"})).as_deref(),
            Some("https://quickbooks.api.intuit.com/v3/company/456")
        );
    }

    fn oauth_cfg_basic_revoke() -> OAuthConfig {
        OAuthConfig {
            auth_url: "",
            token_url: "",
            default_scopes: "",
            token_endpoint_auth: TokenEndpointAuth::HttpBasic,
            refresh_token_rotates: false,
            base_url: "",
            sandbox_base_url: "",
            base_url_path_template: "",
            extra_callback_params: &[],
            reauth_on_error_codes: &[],
            revocation_endpoint: "https://revoke.example.com",
            pkce_required: false,
            params_driven: false,
        }
    }

    #[test]
    fn revoke_request_basic_sends_json_token_and_basic_header() {
        let cfg = oauth_cfg_basic_revoke();
        let params = json!({"client_id":"cid","client_secret":"csec","refresh_token":"rt"});
        let (basic, body, endpoint) = build_revoke_request(&cfg, &params).expect("revoke request");
        assert_eq!(endpoint, "https://revoke.example.com");
        assert_eq!(basic.as_deref(), Some("Basic Y2lkOmNzZWM=")); // base64("cid:csec")
        assert!(body.contains("\"token\":\"rt\""), "body: {body}");
    }

    #[test]
    fn revoke_request_none_without_endpoint_or_token() {
        // No revocation endpoint declared → nothing to do.
        assert!(
            build_revoke_request(&oauth_cfg("", "", ""), &json!({"refresh_token":"rt"})).is_none()
        );
        // Endpoint declared but no token on the connection → nothing to revoke.
        assert!(
            build_revoke_request(&oauth_cfg_basic_revoke(), &json!({"client_id":"cid"})).is_none()
        );
    }

    #[test]
    fn curated_provider_ignores_hostile_endpoint_params() {
        // THE hijack regression: a curated (params_driven=false) descriptor with a
        // legitimately-EMPTY revocation endpoint + hostile params must resolve
        // byte-identically to the static descriptor — params never consulted.
        let cfg = OAuthConfig {
            auth_url: "https://app.hubspot.com/oauth/authorize",
            token_url: "https://api.hubapi.com/oauth/v1/token",
            default_scopes: "oauth",
            token_endpoint_auth: TokenEndpointAuth::FormBody,
            refresh_token_rotates: false,
            base_url: "https://api.hubapi.com",
            sandbox_base_url: "",
            base_url_path_template: "",
            extra_callback_params: &[],
            reauth_on_error_codes: &[],
            revocation_endpoint: "", // legitimately empty on the curated provider
            pkce_required: false,
            params_driven: false,
        };
        let hostile = json!({
            "auth_url": "https://attacker.example/authorize",
            "token_url": "https://attacker.example/token",
            "revocation_url": "https://attacker.example/revoke",
            "base_url": "https://attacker.example",
            "token_auth": "basic",
            "pkce": true,
            "refresh_rotates": true,
            "refresh_token": "rt"
        });
        let eff = resolve_effective_oauth_config(&cfg, &hostile);
        assert_eq!(eff.auth_url, "https://app.hubspot.com/oauth/authorize");
        assert_eq!(eff.token_url, "https://api.hubapi.com/oauth/v1/token");
        assert_eq!(eff.token_endpoint_auth, TokenEndpointAuth::FormBody);
        assert!(!eff.pkce_required);
        assert!(!eff.refresh_token_rotates);
        // The empty descriptor endpoint must NOT be filled from params: no revoke
        // target may be conjured that would receive the refresh token.
        assert!(eff.revocation_endpoint.is_empty());
        assert!(
            build_revoke_request(&cfg, &hostile).is_none(),
            "hostile revocation_url must not create a revoke request"
        );
        // Base URL likewise stays the descriptor's.
        assert_eq!(
            resolve_oauth_base_url(&cfg, &hostile).as_deref(),
            Some("https://api.hubapi.com")
        );
    }

    #[test]
    fn params_driven_type_resolves_from_params() {
        let cfg = OAuthConfig {
            auth_url: "",
            token_url: "",
            default_scopes: "",
            token_endpoint_auth: TokenEndpointAuth::FormBody,
            refresh_token_rotates: false,
            base_url: "",
            sandbox_base_url: "",
            base_url_path_template: "",
            extra_callback_params: &[],
            reauth_on_error_codes: &["invalid_grant"],
            revocation_endpoint: "",
            pkce_required: false,
            params_driven: true,
        };
        let params = json!({
            "auth_url": "https://auth.example.com/authorize",
            "token_url": "https://auth.example.com/token",
            "base_url": "https://api.example.com/",
            "token_auth": "basic",
            "revocation_url": "https://auth.example.com/revoke"
        });
        let eff = resolve_effective_oauth_config(&cfg, &params);
        assert_eq!(eff.auth_url, "https://auth.example.com/authorize");
        assert_eq!(eff.token_url, "https://auth.example.com/token");
        assert_eq!(eff.token_endpoint_auth, TokenEndpointAuth::HttpBasic);
        // Defaults: PKCE on, rotation fail-closed on.
        assert!(eff.pkce_required);
        assert!(eff.refresh_token_rotates);
        assert_eq!(eff.revocation_endpoint, "https://auth.example.com/revoke");
        // Trailing slash trimmed on the pinned base URL.
        assert_eq!(
            resolve_oauth_base_url(&cfg, &params).as_deref(),
            Some("https://api.example.com")
        );
        // Explicit opt-outs are honored for params-driven types only.
        let eff2 = resolve_effective_oauth_config(
            &cfg,
            &json!({"token_url": "https://t", "pkce": false, "refresh_rotates": false}),
        );
        assert!(!eff2.pkce_required);
        assert!(!eff2.refresh_token_rotates);
        // Missing endpoints stay empty -> call sites fail fast.
        let eff3 = resolve_effective_oauth_config(&cfg, &json!({}));
        assert!(eff3.auth_url.is_empty() && eff3.token_url.is_empty());
    }

    #[test]
    fn generic_client_credentials_form_body_puts_creds_in_body() {
        let params = json!({
            "token_url": "https://auth.example.com/token",
            "client_id": "cid",
            "client_secret": "csec",
            "scope": "read write",
            "base_url": "https://api.example.com",
            "token_auth": "form_body",
            "audience": "https://api.example.com"
        });
        let auth = describe_http_oauth2_client_credentials_auth("conn1", &params).unwrap();
        match auth {
            DeferredAuth::OAuth2ClientCredentials {
                token_url,
                basic_auth,
                request_body,
                ..
            } => {
                assert_eq!(token_url, "https://auth.example.com/token");
                assert!(basic_auth.is_none(), "form_body must not set Basic auth");
                match request_body {
                    TokenRequestBody::FormUrlEncoded(fields) => {
                        assert!(fields.contains(&("client_id".to_string(), "cid".to_string())));
                        assert!(
                            fields.contains(&("client_secret".to_string(), "csec".to_string()))
                        );
                        assert!(fields.contains(&("scope".to_string(), "read write".to_string())));
                        assert!(fields.contains(&(
                            "audience".to_string(),
                            "https://api.example.com".to_string()
                        )));
                    }
                    _ => panic!("expected form body"),
                }
            }
            _ => panic!("expected client credentials"),
        }
    }

    #[test]
    fn generic_client_credentials_basic_moves_creds_to_header() {
        let params = json!({
            "token_url": "https://auth.example.com/token",
            "client_id": "cid",
            "client_secret": "csec",
            "base_url": "https://api.example.com",
            "token_auth": "basic"
        });
        let auth = describe_http_oauth2_client_credentials_auth("conn1", &params).unwrap();
        match auth {
            DeferredAuth::OAuth2ClientCredentials {
                basic_auth,
                request_body,
                ..
            } => {
                assert_eq!(
                    basic_auth,
                    Some(("cid".to_string(), "csec".to_string())),
                    "basic style must carry creds for the Authorization header"
                );
                match request_body {
                    TokenRequestBody::FormUrlEncoded(fields) => {
                        // Credentials must be OMITTED from the body under Basic.
                        assert!(!fields.iter().any(|(k, _)| k == "client_id"));
                        assert!(!fields.iter().any(|(k, _)| k == "client_secret"));
                        assert!(fields.contains(&(
                            "grant_type".to_string(),
                            "client_credentials".to_string()
                        )));
                    }
                    _ => panic!("expected form body"),
                }
            }
            _ => panic!("expected client credentials"),
        }
    }

    #[test]
    fn generic_client_credentials_requires_endpoints() {
        // Missing token_url -> no deferred auth (fail fast, no empty-host URL).
        assert!(
            describe_http_oauth2_client_credentials_auth(
                "c",
                &json!({"client_id": "a", "client_secret": "b"})
            )
            .is_none()
        );
    }

    #[test]
    fn collect_shopify_scopes_keeps_enabled_scopes_only() {
        let params = json!({
            "scope_read_products": true,
            "scope_write_products": false,
            "scope_read_inventory": true
        });

        assert_eq!(
            collect_shopify_scopes(&params),
            "read_products,read_inventory"
        );
    }

    #[test]
    fn microsoft_entra_client_credentials_builds_form_token_request() {
        let params = json!({
            "tenant_id": "contoso.onmicrosoft.com",
            "client_id": "client-id",
            "client_secret": "client-secret",
            "scope": "https://api.businesscentral.dynamics.com/.default",
            "base_url": "https://api.businesscentral.dynamics.com/v2.0/production/api/v2.0"
        });
        let mut headers = HashMap::new();

        let descriptor = describe_connection_auth(
            "conn-bc",
            "microsoft_entra_client_credentials",
            &params,
            &mut headers,
        );

        assert_eq!(
            descriptor.base_url.as_deref(),
            Some("https://api.businesscentral.dynamics.com/v2.0/production/api/v2.0")
        );
        assert!(headers.is_empty());

        let auth = descriptor.deferred_auth.expect("deferred auth");
        match auth {
            DeferredAuth::OAuth2ClientCredentials {
                token_url,
                header_name,
                header_value_prefix,
                request_body,
                ..
            } => {
                assert_eq!(
                    token_url,
                    "https://login.microsoftonline.com/contoso.onmicrosoft.com/oauth2/v2.0/token"
                );
                assert_eq!(header_name, "Authorization");
                assert_eq!(header_value_prefix.as_deref(), Some("Bearer "));

                match request_body {
                    TokenRequestBody::FormUrlEncoded(fields) => {
                        assert!(fields.contains(&(
                            "grant_type".to_string(),
                            "client_credentials".to_string()
                        )));
                        assert!(
                            fields.contains(&("client_id".to_string(), "client-id".to_string()))
                        );
                        assert!(
                            fields.contains(&(
                                "client_secret".to_string(),
                                "client-secret".to_string()
                            ))
                        );
                        assert!(fields.contains(&(
                            "scope".to_string(),
                            "https://api.businesscentral.dynamics.com/.default".to_string()
                        )));
                    }
                    TokenRequestBody::Json(_) => panic!("expected form-encoded token body"),
                }
            }
            DeferredAuth::OAuth2RefreshToken { .. } => {
                panic!("expected client credentials auth")
            }
        }
    }

    #[test]
    fn aws_credentials_defaults_to_bedrock_runtime_and_accepts_aws_field_names() {
        let params = json!({
            "aws_access_key_id": "AKIA_TEST",
            "aws_secret_access_key": "secret",
            "aws_region": "eu-central-1",
            "aws_session_token": "token"
        });
        let mut headers = HashMap::new();

        let descriptor = describe_connection_auth("conn", "aws_credentials", &params, &mut headers);

        assert_eq!(
            descriptor.base_url.as_deref(),
            Some("https://bedrock-runtime.eu-central-1.amazonaws.com")
        );
        let aws = descriptor.aws_signing.expect("aws signing params");
        assert_eq!(aws.access_key_id, "AKIA_TEST");
        assert_eq!(aws.secret_access_key, "secret");
        assert_eq!(aws.region, "eu-central-1");
        assert_eq!(aws.service, "bedrock");
        assert_eq!(aws.session_token.as_deref(), Some("token"));
    }

    #[test]
    fn aws_credentials_ignores_blank_optional_session_token() {
        let params = json!({
            "aws_access_key_id": "AKIA_TEST",
            "aws_secret_access_key": "secret",
            "aws_region": "ap-southeast-2",
            "aws_session_token": ""
        });
        let mut headers = HashMap::new();

        let descriptor = describe_connection_auth("conn", "aws_credentials", &params, &mut headers);
        let aws = descriptor.aws_signing.expect("aws signing params");

        assert_eq!(aws.access_key_id, "AKIA_TEST");
        assert_eq!(aws.region, "ap-southeast-2");
        assert!(aws.session_token.is_none());
    }

    #[test]
    fn mcp_bearer_injects_authorization_header() {
        let params = json!({
            "url": "https://mcp.example.com/jsonrpc",
            "auth_mode": "bearer",
            "bearer_token": "tkn_abc"
        });
        let mut headers = HashMap::new();
        let descriptor = describe_connection_auth("c", "mcp", &params, &mut headers);
        assert_eq!(
            headers.get("Authorization"),
            Some(&"Bearer tkn_abc".to_string())
        );
        assert_eq!(
            descriptor.base_url.as_deref(),
            Some("https://mcp.example.com/jsonrpc")
        );
    }

    #[test]
    fn mcp_api_key_injects_custom_header() {
        let params = json!({
            "url": "https://mcp.example.com/jsonrpc",
            "auth_mode": "api_key",
            "api_key_header": "X-Linear-Token",
            "api_key_value": "lin_xyz"
        });
        let mut headers = HashMap::new();
        let _ = describe_connection_auth("c", "mcp", &params, &mut headers);
        assert_eq!(headers.get("X-Linear-Token"), Some(&"lin_xyz".to_string()));
        assert!(!headers.contains_key("Authorization"));
    }

    #[test]
    fn mcp_api_key_defaults_to_x_api_key_header() {
        let params = json!({
            "url": "https://mcp.example.com/jsonrpc",
            "auth_mode": "api_key",
            "api_key_value": "secret"
        });
        let mut headers = HashMap::new();
        let _ = describe_connection_auth("c", "mcp", &params, &mut headers);
        assert_eq!(headers.get("X-API-Key"), Some(&"secret".to_string()));
    }

    #[test]
    fn mcp_none_injects_no_auth_header() {
        let params = json!({
            "url": "https://mcp.example.com/jsonrpc",
            "auth_mode": "none"
        });
        let mut headers = HashMap::new();
        let _ = describe_connection_auth("c", "mcp", &params, &mut headers);
        assert!(!headers.contains_key("Authorization"));
        assert!(!headers.contains_key("X-API-Key"));
    }

    #[test]
    fn mcp_extra_headers_are_forwarded() {
        let params = json!({
            "url": "https://mcp.example.com/jsonrpc",
            "auth_mode": "none",
            "extra_headers": {"X-Custom": "yes", "Mcp-Session-Id": "sess1"}
        });
        let mut headers = HashMap::new();
        let _ = describe_connection_auth("c", "mcp", &params, &mut headers);
        assert_eq!(headers.get("X-Custom"), Some(&"yes".to_string()));
        assert_eq!(headers.get("Mcp-Session-Id"), Some(&"sess1".to_string()));
    }
}
