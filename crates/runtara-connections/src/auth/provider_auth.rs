use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use reqwest::Client;
use runtara_agents::registry::find_connection_type;
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
        "hubspot_private_app" => {
            if let Some(auth) =
                describe_oauth_refresh_auth(connection_id, integration_id, params, "Authorization")
            {
                ConnectionAuthDescriptor {
                    base_url: Some("https://api.hubapi.com".into()),
                    aws_signing: None,
                    azure_signing: None,
                    deferred_auth: Some(auth),
                }
            } else {
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

    Some(DeferredAuth::OAuth2RefreshToken {
        cache_key: token_cache::build_token_cache_key(&[
            "oauth_refresh",
            connection_id,
            integration_id,
        ]),
        token_url: oauth_config.token_url.to_string(),
        header_name: header_name.to_string(),
        client_id,
        client_secret,
        refresh_token,
        token_endpoint_auth: oauth_config.token_endpoint_auth,
        fallback_access_token: params["access_token"].as_str().map(|s| s.to_string()),
        fallback_expires_at: token_cache::parse_expiry(params["token_expires_at"].as_str()),
    })
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
    use serde_json::json;

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
