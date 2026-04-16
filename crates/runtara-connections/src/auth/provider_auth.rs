use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use reqwest::Client;
use runtara_dsl::agent_meta::find_connection_type;
use serde_json::Value;
use std::collections::HashMap;

use super::aws_signing::AwsSigningParams;
use super::token_cache::{
    self, DEFAULT_CLIENT_CREDENTIALS_TTL_SECONDS, DeferredAuth, TokenRequestBody,
};

pub struct ResolvedConnectionAuth {
    pub base_url: Option<String>,
    pub aws_signing: Option<AwsSigningParams>,
}

pub(crate) struct ConnectionAuthDescriptor {
    pub base_url: Option<String>,
    pub aws_signing: Option<AwsSigningParams>,
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
) -> Result<ResolvedConnectionAuth, String> {
    let descriptor = describe_connection_auth(connection_id, integration_id, params, headers);

    if let Some(deferred_auth) = descriptor.deferred_auth {
        let resolved = token_cache::resolve_deferred_auth(client, deferred_auth).await?;
        headers.insert(resolved.0, resolved.1);
    }

    Ok(ResolvedConnectionAuth {
        base_url: descriptor.base_url,
        aws_signing: descriptor.aws_signing,
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
                deferred_auth: None,
            }
        }
        "shopify_client_credentials" => ConnectionAuthDescriptor {
            base_url: params["shop_domain"]
                .as_str()
                .map(|domain| format!("https://{}", domain)),
            aws_signing: None,
            deferred_auth: describe_shopify_client_credentials_auth(connection_id, params),
        },
        "hubspot_access_token" => {
            if let Some(token) = params["access_token"].as_str() {
                headers.insert("Authorization".into(), format!("Bearer {}", token));
            }
            ConnectionAuthDescriptor {
                base_url: Some("https://api.hubapi.com".into()),
                aws_signing: None,
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
                    deferred_auth: Some(auth),
                }
            } else {
                if let Some(token) = params["access_token"].as_str() {
                    headers.insert("Authorization".into(), format!("Bearer {}", token));
                }
                ConnectionAuthDescriptor {
                    base_url: Some("https://api.hubapi.com".into()),
                    aws_signing: None,
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
                deferred_auth: None,
            }
        }
        "aws_credentials" | "s3_compatible" => {
            let access_key_id = params["access_key_id"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let secret_access_key = params["secret_access_key"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let region = params["region"].as_str().unwrap_or("us-east-1").to_string();
            let session_token = params["session_token"].as_str().map(|s| s.to_string());

            let (base_url, service) = if integration_id == "s3_compatible" {
                let endpoint = params["endpoint"]
                    .as_str()
                    .map(normalize_endpoint)
                    .unwrap_or_else(|| format!("https://s3.{}.amazonaws.com", region));
                (Some(endpoint), "s3".to_string())
            } else {
                let svc = params["service"].as_str().unwrap_or("s3").to_string();
                let base = params["endpoint"].as_str().map(normalize_endpoint);
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
        request_body: TokenRequestBody::Json(Value::Object(body)),
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
    let token_url = find_connection_type(integration_id)?
        .oauth_config
        .map(|config| config.token_url.to_string())?;

    Some(DeferredAuth::OAuth2RefreshToken {
        cache_key: token_cache::build_token_cache_key(&[
            "oauth_refresh",
            connection_id,
            integration_id,
        ]),
        token_url,
        header_name: header_name.to_string(),
        client_id,
        client_secret,
        refresh_token,
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

fn normalize_endpoint(endpoint: &str) -> String {
    if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
        endpoint.to_string()
    } else {
        format!("https://{}", endpoint)
    }
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
}
